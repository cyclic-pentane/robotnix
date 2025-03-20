use std::collections::{HashMap, HashSet};
use std::fs;
use std::fs::File;
use std::io;
use std::io::Write;
use std::env;
use std::io::{BufReader, BufWriter};
use std::process::Command;
use serde::{Deserialize, Serialize};
use serde_json;

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
struct Device {
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
    LsRemote(io::Error),
    Parser,
}

fn get_rev_of_branch(repo: &Repository, branch: &str) -> Result<String, GetRevOfBranchError> {
    let url = repo.url();
    println!("ls-remote'ing {}", &url);
    let output = Command::new("git")
        .arg("ls-remote")
        .arg(&url)
        .arg(format!("refs/heads/{branch}"))
        .output()
        .map_err(|e| GetRevOfBranchError::LsRemote(e))?
        .stdout;
    Ok(std::str::from_utf8(output.split(|x| x == &b'\t').nth(0)
        .ok_or(GetRevOfBranchError::Parser)?)
        .map_err(|_| GetRevOfBranchError::Parser)?
        .to_string())
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
    FileWrite(io::Error),
}

fn fetch_device_metadata_to(device_metadata_path: &str) -> Result<(), FetchDeviceMetadataToError> {
    let fetched_device_metadata = fetch_device_metadata()
        .map_err(|e| FetchDeviceMetadataToError::Fetch(e))?;
    let file = File::create(device_metadata_path)
        .map_err(|e| FetchDeviceMetadataToError::FileWrite(e))?;
    let writer = BufWriter::new(file);
    serde_json::to_writer(writer, &fetched_device_metadata);

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

fn read_device_dir_file(path: &str) -> Result<HashMap<String, Device>, ReadDeviceDirsError> {
    let file = File::open(path).map_err(|e| ReadDeviceDirsError::ReadFile(e))?;
    let reader = BufReader::new(file);
    
    serde_json::from_reader(reader).map_err(|e| ReadDeviceDirsError::Parser(e))
}


#[derive(Debug)]
enum FetchDeviceDirsError {
    ReadDeviceDirs(ReadDeviceDirsError),
    GetRevOfBranch(GetRevOfBranchError),
    PrefetchGit(NixPrefetchGitError),
    WriteToFile(io::Error),
    Serialize(serde_json::Error),
}

fn incrementally_fetch_device_dirs(devices: &HashMap<String, DeviceMetadata>, device_dirs_path: &str) -> Result<HashMap<String, Device>, FetchDeviceDirsError> {
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
            device_dirs.insert(device_name.clone(), Device {
                deps: HashMap::new(),
            });
        }
        let mut device = device_dirs.get_mut(device_name).unwrap();

        for dep in device_metadata.deps.iter() {
            let path = dep.source_tree_path();
            let is_up_to_date = if let Some(args) = device.deps.get(&path) {
                let current_rev = get_rev_of_branch(dep, &device_metadata.branch)
                    .map_err(|e| FetchDeviceDirsError::GetRevOfBranch(e))?;
                current_rev == args.rev
            } else {
                false
            };

            if !is_up_to_date {
                let output = nix_prefetch_git_repo(dep)
                    .map_err(|e| FetchDeviceDirsError::PrefetchGit(e))?;
                device.deps.insert(path, FetchgitArgs::from_prefetch_output(output));
            }
        }
        let device_dirs_json = serde_json::to_string(&device_dirs)
            .map_err(|e| FetchDeviceDirsError::Serialize(e))?;
        let mut device_dirs_file = File::create(device_dirs_path)
            .map_err(|e| FetchDeviceDirsError::WriteToFile(e))?;
        device_dirs_file.write_all(device_dirs_json.as_bytes());
    }

    Ok(device_dirs)
}


fn main() {
    let args: Vec<String> = env::args().collect();
    let device_metadata_path = &args[1];
    let device_dirs_path = &args[2];

    fetch_device_metadata_to(device_metadata_path).unwrap();

    let devices = read_device_metadata(device_metadata_path).unwrap();
    let device_dirs = incrementally_fetch_device_dirs(&devices, device_dirs_path).unwrap();
}
