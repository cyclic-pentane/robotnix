use std::collections::{HashMap, HashSet};
use std::fs;
use std::fs::File;
use atomic_write_file::AtomicWriteFile;
use std::io;
use std::io::Write;
use std::env;
use std::io::{BufReader, BufWriter};
use std::process::Command;
use serde::{Deserialize, Serialize};
use serde_json;
use clap::Parser;
use git2;

#[derive(Debug, Serialize, Deserialize)]
enum Variant {
    #[serde(rename = "userdebug")]
    Userdebug,
}

#[derive(Debug, Serialize, Deserialize)]
struct DeviceMetadata {
    branch: String,
    vendor: String,
    name: String,
    variant: Variant,
    deps: Vec<Repository>
}

#[derive(Debug, Serialize, Deserialize)]
struct NixPrefetchGitOutput {
    url: String,
    rev: String,
    date: String,
    path: String,
    hash: String,
    #[serde(rename = "fetchLFS")]
    fetch_lfs: bool,
    #[serde(rename = "fetchSubmodules")]
    fetch_submodules: bool,
    #[serde(rename = "deepClone")]
    deep_clone: bool,
    #[serde(rename = "leaveDotGit")]
    leave_dot_git: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct FetchgitArgs {
    url: String,
    rev: String,
    hash: String,
    #[serde(rename = "fetchLFS")]
    fetch_lfs: bool,
    #[serde(rename = "fetchSubmodules")]
    fetch_submodules: bool,
    #[serde(rename = "deepClone")]
    deep_clone: bool,
    #[serde(rename = "leaveDotGit")]
    leave_dot_git: bool,
}

impl FetchgitArgs {
    fn from_prefetch_output(output: NixPrefetchGitOutput) -> FetchgitArgs {
        FetchgitArgs {
            url: output.url,
            rev: output.rev,
            hash: output.hash,
            fetch_lfs: output.fetch_lfs,
            fetch_submodules: output.fetch_submodules,
            deep_clone: output.deep_clone,
            leave_dot_git: output.leave_dot_git,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct DeviceDir {
    deps: HashMap<String, FetchgitArgs>,
}

#[derive(Debug, Serialize, Deserialize)]
enum Remote {
    LineageOS,
    TheMuppetsGitHub,
    TheMuppetsGitLab
}

#[derive(Debug, Serialize, Deserialize)]
struct Repository {
    remote: Remote,
    path: Vec<String>
}

impl Remote {
    fn base_url(&self) -> String {
        match self {
            Remote::LineageOS => "https://github.com/LineageOS",
            Remote::TheMuppetsGitHub => "https://github.com/TheMuppets",
            Remote::TheMuppetsGitLab => "https://gitlab.com/TheMuppets"
        }.to_string()
    }
}

impl Repository {
    fn new_device_repo(vendor: &str, device: &str) -> Repository {
        Repository {
            remote: Remote::LineageOS,
            path: vec![
                "android".to_string(),
                "device".to_string(),
                vendor.to_string(),
                device.to_string()
            ]
        }
    }

    fn url(&self) -> String {
        format!("{}/{}", &self.remote.base_url(), &self.path.join("_"))
    }

    // Path of the git repository within the AOSP source tree. For instance,
    // android_device_fairphone_FP4 has the source tree path device/fairphone/FP4
    fn source_tree_path(&self) -> String {
        match self.path.get(0).map(|x| x.as_str()) {
            Some("android") => &self.path[1..],
            Some("proprietary") => &self.path[1..],
            Some(_) => panic!("Not implemented yet"),
            None => panic!("Empty path")
        }.join("/")
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct HudsonDevice {
    model: String,
    oem: String,
    name: String,
}

#[derive(Debug)]
enum DeviceMetadataHudsonError {
    HudsonFileRead(io::Error),
    Utf8(std::str::Utf8Error),
    InvalidLineageBuildTargets,
    Parser(serde_json::Error),
    ModelNotFoundInUpdaterDir(String),
}

fn get_device_metadata_from_hudson(hudson_path: &str) -> Result<HashMap<String, DeviceMetadata>, DeviceMetadataHudsonError> {
    let build_targets = {
        let text_bytes = fs::read(format!("{}/lineage-build-targets", hudson_path))
            .map_err(|e| DeviceMetadataHudsonError::HudsonFileRead(e))?;
        let text = std::str::from_utf8(&text_bytes)
            .map_err(|e| DeviceMetadataHudsonError::Utf8(e))?;
        let mut build_targets = vec![];
        for line in text.split("\n") {
            if line.starts_with("#") || line == "" {
                continue;
            }
            let fields: Vec<&str> = line.split(" ").collect();
            let device  = fields.get(0).ok_or(DeviceMetadataHudsonError::InvalidLineageBuildTargets)?.to_string();
            let variant = fields.get(1).ok_or(DeviceMetadataHudsonError::InvalidLineageBuildTargets)?.to_string();
            let branch  = fields.get(2).ok_or(DeviceMetadataHudsonError::InvalidLineageBuildTargets)?.to_string();

            build_targets.push((device, variant, branch));
        }
        
        build_targets
    };

    let reader = BufReader::new(File::open(format!("{}/updater/devices.json", hudson_path))
        .map_err(|e| DeviceMetadataHudsonError::HudsonFileRead(e))?);
    let hudson_devices: Vec<HudsonDevice> = serde_json::from_reader(reader)
        .map_err(|e| DeviceMetadataHudsonError::Parser(e))?;
    let reader = BufReader::new(File::open(format!("{}/updater/device_deps.json", hudson_path))
        .map_err(|e| DeviceMetadataHudsonError::HudsonFileRead(e))?);
    let hudson_device_deps: HashMap<String, Vec<String>> = serde_json::from_reader(reader)
        .map_err(|e| DeviceMetadataHudsonError::Parser(e))?;

    let mut device_metadata = HashMap::new();

    for (device, variant, branch) in build_targets {
        let hudson_device = hudson_devices.iter().filter(|x| x.model == device).next().ok_or(DeviceMetadataHudsonError::ModelNotFoundInUpdaterDir(device.clone()))?;
        let hudson_deps = hudson_device_deps.get(&device).ok_or(DeviceMetadataHudsonError::ModelNotFoundInUpdaterDir(device.clone()))?;
        device_metadata.insert(device, DeviceMetadata { 
            name: hudson_device.name.clone(),
            branch: branch,
            // We use the json parser for strings like `userdebug` by wrapping them in quotation
            // marks, like `"userdebug"`. This is a dirty hack and I need to figure out how to do
            // this properly at some point.
            variant: serde_json::from_str(&format!("\"{}\"", variant)).map_err(|e| DeviceMetadataHudsonError::Parser(e))?,
            vendor: hudson_device.oem.to_lowercase(),
            deps: hudson_deps.iter().map(|x| Repository {
                remote: Remote::LineageOS,
                path: x.split("_").map(|x| x.to_string()).collect(),
            }).collect()
        });
    }

    Ok(device_metadata)
}

#[derive(Debug)]
enum FetchDeviceMetadataError {
    PrefetchGit(NixPrefetchGitError),
    Hudson(DeviceMetadataHudsonError),
}

fn fetch_device_metadata() -> Result<HashMap<String, DeviceMetadata>, FetchDeviceMetadataError> {
    let prefetch_git_output = nix_prefetch_git_repo(&Repository {
        remote: Remote::LineageOS,
        path: vec!["hudson".to_string()],
    }).map_err(|e| FetchDeviceMetadataError::PrefetchGit(e))?;

    get_device_metadata_from_hudson(&prefetch_git_output.path)
        .map_err(|e| FetchDeviceMetadataError::Hudson(e))
}

#[derive(Debug)]
enum GetRevOfBranchError {
    Libgit(git2::Error),
    BranchNotFound,
}

fn get_rev_of_branch(repo: &Repository, branch: &str) -> Result<String, GetRevOfBranchError> {
    let mut remote = git2::Remote::create_detached(repo.url())
        .map_err(|e| GetRevOfBranchError::Libgit(e))?;
    remote.connect(git2::Direction::Fetch);
    let list_result = remote.list()
        .map_err(|e| GetRevOfBranchError::Libgit(e))?;
    for remote_head in list_result.iter() {
        if remote_head.name() == format!("refs/heads/{branch}") {
            return Ok(format!("{:?}", remote_head.oid()))
        }
    }
    Err(GetRevOfBranchError::BranchNotFound)
}


#[derive(Debug)]
enum NixPrefetchGitError {
    IOError(io::Error),
    Parser(serde_json::Error),
}

fn nix_prefetch_git_repo(repo: &Repository) -> Result<NixPrefetchGitOutput, NixPrefetchGitError> {
    let repo_url = repo.url();
    println!("Prefetching {}", &repo_url);
    let output = Command::new("nix-prefetch-git")
        .arg(&repo_url)
        .output()
        .map_err(|e| NixPrefetchGitError::IOError(e))?;

    serde_json::from_slice(&output.stdout).map_err(|e| NixPrefetchGitError::Parser(e))
}

#[derive(Debug)]
enum FetchDeviceMetadataToError {
    Fetch(FetchDeviceMetadataError),
    Parser(serde_json::Error),
    FileWrite(io::Error),
}

fn fetch_device_metadata_to(device_metadata_path: &str) -> Result<(), FetchDeviceMetadataToError> {
    let fetched_device_metadata = fetch_device_metadata()
        .map_err(|e| FetchDeviceMetadataToError::Fetch(e))?;
    let mut file = AtomicWriteFile::options()
        .open(device_metadata_path)
        .map_err(|e| FetchDeviceMetadataToError::FileWrite(e))?;
    let buf = serde_json::to_string(&fetched_device_metadata)
        .map_err(|e| FetchDeviceMetadataToError::Parser(e))?;

    file.write(buf.as_bytes());
    file.commit();

    Ok(())
}

#[derive(Debug)]
enum ReadDeviceMetadataError {
    ReadFile(io::Error),
    Parser(serde_json::Error),
}

fn read_device_metadata(path: &str) -> Result<HashMap<String, DeviceMetadata>, ReadDeviceMetadataError> {
    let file = File::open(path).map_err(|e| ReadDeviceMetadataError::ReadFile(e))?;
    let reader = BufReader::new(file);

    serde_json::from_reader(reader).map_err(|e| ReadDeviceMetadataError::Parser(e))
}

#[derive(Debug)]
enum ReadDeviceDirsError {
    ReadFile(io::Error),
    Parser(serde_json::Error),
}

fn read_device_dir_file(path: &str) -> Result<HashMap<String, DeviceDir>, ReadDeviceDirsError> {
    let file = File::open(path).map_err(|e| ReadDeviceDirsError::ReadFile(e))?;
    let reader = BufReader::new(file);
    
    serde_json::from_reader(reader).map_err(|e| ReadDeviceDirsError::Parser(e))
}

#[derive(Debug)]
enum WriteDeviceDirsError {
    Serialize(serde_json::Error),
    WriteToFile(io::Error),
}

fn write_device_dir_file(path: &str, device_dirs: &HashMap<String, DeviceDir>) -> Result<(), WriteDeviceDirsError> {
    let device_dirs_json = serde_json::to_string(&device_dirs)
        .map_err(|e| WriteDeviceDirsError::Serialize(e))?;
    let mut device_dirs_file = AtomicWriteFile::options().open(path)
        .map_err(|e| WriteDeviceDirsError::WriteToFile(e))?;
    device_dirs_file.write_all(device_dirs_json.as_bytes())
        .map_err(|e| WriteDeviceDirsError::WriteToFile(e))?;
    device_dirs_file.commit();

    Ok(())
}


#[derive(Debug)]
enum FetchDeviceDirsError {
    ReadDeviceDirs(ReadDeviceDirsError),
    GetRevOfBranch(GetRevOfBranchError),
    PrefetchGit(NixPrefetchGitError),
    WriteFile(WriteDeviceDirsError),
}

fn incrementally_fetch_device_dirs(devices: &HashMap<String, DeviceMetadata>, device_dirs_path: &str) -> Result<HashMap<String, DeviceDir>, FetchDeviceDirsError> {
    let mut device_dirs = match read_device_dir_file(device_dirs_path) {
        Ok(dirs) => dirs,
        Err(ReadDeviceDirsError::ReadFile(_)) => {
            println!("Could not open {}, starting from scratch...", device_dirs_path);
            HashMap::new()
        },
        Err(e) => return Err(FetchDeviceDirsError::ReadDeviceDirs(e)),
    };

    for (device_name, device_metadata) in devices.iter() {
        if !device_dirs.contains_key(device_name) {
            device_dirs.insert(device_name.clone(), DeviceDir {
                deps: HashMap::new(),
            });
        }

        for dep in device_metadata.deps.iter() {
            let path = dep.source_tree_path();
            let is_up_to_date = {
                let device = device_dirs.get(device_name).unwrap();

                if let Some(args) = device.deps.get(&path) {
                    let current_rev = get_rev_of_branch(dep, &device_metadata.branch)
                        .map_err(|e| FetchDeviceDirsError::GetRevOfBranch(e))?;
                    current_rev == args.rev
                } else {
                    false
                }
            };

            if !is_up_to_date {
                let output = nix_prefetch_git_repo(dep)
                    .map_err(|e| FetchDeviceDirsError::PrefetchGit(e))?;
                device_dirs
                    .get_mut(device_name)
                    .unwrap()
                    .deps
                    .insert(path, FetchgitArgs::from_prefetch_output(output));
            }

            write_device_dir_file(device_dirs_path, &device_dirs)
                .map_err(|e| FetchDeviceDirsError::WriteFile(e))?;
        }
    }

    Ok(device_dirs)
}

fn incrementally_fetch_vendor_dirs(devices: &HashMap<String, DeviceMetadata>, device_dirs: &HashMap<String, DeviceDir>, vendor_dirs_path: &str) -> Result<HashMap<String, DeviceDir>, FetchDeviceDirsError> {
    let mut vendor_dirs = match read_device_dir_file(vendor_dirs_path) {
        Ok(d) => d,
        Err(ReadDeviceDirsError::ReadFile(_)) => {
            println!("Could not open {}, starting from scratch...", vendor_dirs_path);
            HashMap::new()
        },
        Err(e) => return Err(FetchDeviceDirsError::ReadDeviceDirs(e)),
    };

    Ok(vendor_dirs)
}

#[derive(Debug, Parser)]
struct Args {
    #[arg(long)]
    device_metadata_file: String,

    #[arg(long)]
    device_dirs_file: Option<String>,
    
    #[arg(long)]
    vendor_dirs_file: Option<String>,

    #[arg(long)]
    fetch_device_metadata: bool,

    #[arg(long)]
    fetch_device_dirs: bool,

    #[arg(long)]
    fetch_vendor_dirs: bool,
}

fn main() {
    let args = Args::parse();

    if args.fetch_device_metadata {
        fetch_device_metadata_to(&args.device_metadata_file).unwrap()
    }

    if args.fetch_device_dirs {
        let devices = read_device_metadata(&args.device_metadata_file).unwrap();
        incrementally_fetch_device_dirs(
            &devices,
            args.device_dirs_file.as_ref().expect(&"You need to set --device-dirs-file to specify the location to store the device dirs JSON to")
        ).unwrap();
    };

    if args.fetch_vendor_dirs {
        let devices = read_device_metadata(&args.device_metadata_file).unwrap();
        let device_dirs = read_device_dir_file(
            args.device_dirs_file.as_ref().expect(&"You need to set --device-dirs-file to fetch the corresponding vendor dirs")
        ).unwrap();
        incrementally_fetch_vendor_dirs(
            &devices,
            &device_dirs,
            args.vendor_dirs_file.as_ref().expect(&"You need set --vendor-dirs-file to specify the location to store the vendor dirs JSON to")
        );
    }
}
