use std::collections::HashMap;
use std::str;
use std::fs;
use std::fs::File;
use atomic_write_file::AtomicWriteFile;
use std::io;
use std::io::Write;
use std::io::BufReader;
use serde::{Serialize, Deserialize};
use serde_json;
use fast_xml;

use crate::base::{
    Variant,
    Repository,
    Remote,
    GetRevOfBranchError,
    NixPrefetchGitError,
    nix_prefetch_git_repo,
    FetchgitArgs,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct DeviceMetadata {
    branch: String,
    vendor: String,
    name: String,
    variant: Variant,
    deps: Vec<Repository>,
    proprietary_deps: Vec<Repository>
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DeviceDir {
    deps: HashMap<String, FetchgitArgs>,
}

#[derive(Debug, Serialize, Deserialize)]
struct HudsonDevice {
    model: String,
    oem: String,
    name: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename = "project")]
struct MuppetsRepo {
    name: String,
    path: String,
    groups: String,
    #[serde(rename = "clone-depth")]
    clone_depth: u32,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename = "manifest")]
struct MuppetsManifest {
    #[serde(rename = "project", default)]
    projects: Vec<MuppetsRepo>,
}

fn get_proprietary_repos_for_device(muppets_manifests: &MuppetsManifest, device: &str) -> Vec<Repository> {
    let mut repos = vec![];
    for entry in muppets_manifests.projects.iter() {
        let mut found = false;
        for m_group in entry.groups.split(",") {
            if m_group == format!("muppets_{device}") {
                found = true;
                break;
            }
        }
        if found {
            let mut path = vec!["proprietary".to_string()];
            for c in entry.path.split("/") {
                path.push(c.to_string());
            }
            repos.push(Repository {
                remote: Remote::TheMuppetsGitHub,
                path: path,
            });
        }
    }

    repos
}

#[derive(Debug)]
enum GetDeviceMetadataError {
    FileRead(io::Error),
    ParseMuppetsManifest(fast_xml::de::DeError),
    Utf8(std::str::Utf8Error),
    InvalidLineageBuildTargets,
    Parser(serde_json::Error),
    ModelNotFoundInUpdaterDir(String),
}

fn get_device_metadata(hudson_path: &str, muppets_manifests_path: &str) -> Result<HashMap<String, DeviceMetadata>, GetDeviceMetadataError> {
    let muppets_manifest_xml = fs::read(format!("{muppets_manifests_path}/muppets.xml"))
        .map_err(|e| GetDeviceMetadataError::FileRead(e))?;
    let muppets_manifest: MuppetsManifest = fast_xml::de::from_str(
        &str::from_utf8(&muppets_manifest_xml).map_err(|e| GetDeviceMetadataError::Utf8(e))?
    ).map_err(|e| GetDeviceMetadataError::ParseMuppetsManifest(e))?;

    let build_targets = {
        let text_bytes = fs::read(format!("{hudson_path}/lineage-build-targets"))
            .map_err(|e| GetDeviceMetadataError::FileRead(e))?;
        let text = std::str::from_utf8(&text_bytes)
            .map_err(|e| GetDeviceMetadataError::Utf8(e))?;
        let mut build_targets = vec![];
        for line in text.split("\n") {
            if line.starts_with("#") || line == "" {
                continue;
            }
            let fields: Vec<&str> = line.split(" ").collect();
            let device  = fields.get(0).ok_or(GetDeviceMetadataError::InvalidLineageBuildTargets)?.to_string();
            let variant = fields.get(1).ok_or(GetDeviceMetadataError::InvalidLineageBuildTargets)?.to_string();
            let branch  = fields.get(2).ok_or(GetDeviceMetadataError::InvalidLineageBuildTargets)?.to_string();

            build_targets.push((device, variant, branch));
        }
        
        build_targets
    };

    let reader = BufReader::new(File::open(format!("{}/updater/devices.json", hudson_path))
        .map_err(|e| GetDeviceMetadataError::FileRead(e))?);
    let hudson_devices: Vec<HudsonDevice> = serde_json::from_reader(reader)
        .map_err(|e| GetDeviceMetadataError::Parser(e))?;
    let reader = BufReader::new(File::open(format!("{}/updater/device_deps.json", hudson_path))
        .map_err(|e| GetDeviceMetadataError::FileRead(e))?);
    let hudson_device_deps: HashMap<String, Vec<String>> = serde_json::from_reader(reader)
        .map_err(|e| GetDeviceMetadataError::Parser(e))?;

    let mut device_metadata = HashMap::new();

    for (device, variant, branch) in build_targets {
        let hudson_device = hudson_devices.iter().filter(|x| x.model == device).next().ok_or(GetDeviceMetadataError::ModelNotFoundInUpdaterDir(device.clone()))?;
        let mut hudson_deps = hudson_device_deps.get(&device).ok_or(GetDeviceMetadataError::ModelNotFoundInUpdaterDir(device.clone()))?.clone();
        hudson_deps.sort();
        device_metadata.insert(device.clone(), DeviceMetadata { 
            name: hudson_device.name.clone(),
            branch: branch,
            // We use the json parser for strings like `userdebug` by wrapping them in quotation
            // marks, like `"userdebug"`. This is a dirty hack and I need to figure out how to do
            // this properly at some point.
            variant: serde_json::from_str(&format!("\"{}\"", variant)).map_err(|e| GetDeviceMetadataError::Parser(e))?,
            vendor: hudson_device.oem.to_lowercase(),
            deps: hudson_deps.iter().map(|x| Repository {
                remote: Remote::LineageOS,
                path: x.split("_").map(|x| x.to_string()).collect(),
            }).collect(),
            proprietary_deps: get_proprietary_repos_for_device(&muppets_manifest, &device),
        });
    }

    Ok(device_metadata)
}

#[derive(Debug)]
pub enum FetchDeviceMetadataError {
    PrefetchGit(NixPrefetchGitError),
    Hudson(GetDeviceMetadataError),
    Parser(serde_json::Error),
    FileWrite(io::Error),
}

pub fn fetch_device_metadata_to(device_metadata_path: &str, branch: &str) -> Result<(), FetchDeviceMetadataError> {
    let hudson = nix_prefetch_git_repo(&Repository {
        remote: Remote::LineageOS,
        path: vec!["hudson".to_string()],
    }, &"main", None).map_err(|e| FetchDeviceMetadataError::PrefetchGit(e))?;

    let muppets_manifests = nix_prefetch_git_repo(&Repository {
        remote: Remote::TheMuppetsGitHub,
        path: vec!["manifests".to_string()],
    }, branch, None).map_err(|e| FetchDeviceMetadataError::PrefetchGit(e))?;

    let metadata = get_device_metadata(&hudson.path(), &muppets_manifests.path())
        .map_err(|e| FetchDeviceMetadataError::Hudson(e))?;
    let mut file = AtomicWriteFile::options()
        .open(device_metadata_path)
        .map_err(|e| FetchDeviceMetadataError::FileWrite(e))?;
    let buf = serde_json::to_string_pretty(&metadata)
        .map_err(|e| FetchDeviceMetadataError::Parser(e))?;

    file.write(buf.as_bytes());
    file.commit();

    Ok(())
}

#[derive(Debug)]
pub enum ReadDeviceMetadataError {
    ReadFile(io::Error),
    Parser(serde_json::Error),
}

pub fn read_device_metadata(path: &str) -> Result<HashMap<String, DeviceMetadata>, ReadDeviceMetadataError> {
    let file = File::open(path).map_err(|e| ReadDeviceMetadataError::ReadFile(e))?;
    let reader = BufReader::new(file);

    serde_json::from_reader(reader).map_err(|e| ReadDeviceMetadataError::Parser(e))
}

#[derive(Debug)]
pub enum ReadDeviceDirsError {
    ReadFile(io::Error),
    Parser(serde_json::Error),
}

pub fn read_device_dir_file(path: &str) -> Result<HashMap<String, Option<DeviceDir>>, ReadDeviceDirsError> {
    let file = File::open(path).map_err(|e| ReadDeviceDirsError::ReadFile(e))?;
    let reader = BufReader::new(file);
    
    serde_json::from_reader(reader).map_err(|e| ReadDeviceDirsError::Parser(e))
}

#[derive(Debug)]
pub enum WriteDeviceDirsError {
    Serialize(serde_json::Error),
    WriteToFile(io::Error),
}

pub fn write_device_dir_file(path: &str, device_dirs: &HashMap<String, Option<DeviceDir>>) -> Result<(), WriteDeviceDirsError> {
    let device_dirs_json = serde_json::to_string_pretty(&device_dirs)
        .map_err(|e| WriteDeviceDirsError::Serialize(e))?;
    let mut device_dirs_file = AtomicWriteFile::options().open(path)
        .map_err(|e| WriteDeviceDirsError::WriteToFile(e))?;
    device_dirs_file.write_all(device_dirs_json.as_bytes())
        .map_err(|e| WriteDeviceDirsError::WriteToFile(e))?;
    device_dirs_file.commit();

    Ok(())
}


#[derive(Debug)]
pub enum FetchDeviceDirsError {
    ReadDeviceDirs(ReadDeviceDirsError),
    GetRevOfBranch(GetRevOfBranchError),
    PrefetchGit(NixPrefetchGitError),
    WriteFile(WriteDeviceDirsError),
}

pub fn incrementally_fetch_device_dirs(devices: &HashMap<String, DeviceMetadata>, branch: &str, device_dirs_path: &str) -> Result<HashMap<String, Option<DeviceDir>>, FetchDeviceDirsError> {
    let mut device_dirs = match read_device_dir_file(device_dirs_path) {
        Ok(dirs) => dirs,
        Err(ReadDeviceDirsError::ReadFile(_)) => {
            println!("Could not open {}, starting from scratch...", device_dirs_path);
            HashMap::new()
        },
        Err(e) => return Err(FetchDeviceDirsError::ReadDeviceDirs(e)),
    };

    let mut device_names: Vec<&str> = devices.keys().map(|x| x.as_ref()).collect();
    device_names.sort();

    for device_name in device_names {
        println!("At device {device_name}");
        let device_metadata = devices.get(device_name).unwrap();

        if !device_dirs.contains_key(device_name) {
            device_dirs.insert(device_name.to_string(), Some(DeviceDir {
                deps: HashMap::new(),
            }));
        }

        let mut branch_present_on_all_repos = true;
        for dep in device_metadata.deps.iter() {
            let fetchgit_args = match nix_prefetch_git_repo(
                dep,
                branch,
                device_dirs
                    .get(device_name)
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .deps
                    .get(&dep.source_tree_path())
                    .cloned()
            ) {
                Ok(val) => val,
                Err(NixPrefetchGitError::GetRevOfBranch(GetRevOfBranchError::BranchNotFound)) => {
                    // TODO deduplicate this ls-remote operation
                    println!("Branch {branch} not present in repository {dep:?}, skipping device {device_name}");
                    branch_present_on_all_repos = false;
                    break;
                },
                Err(e) => return Err(FetchDeviceDirsError::PrefetchGit(e)),
            };

            device_dirs
                .get_mut(device_name)
                .unwrap()
                .as_mut()
                .unwrap()
                .deps
                .insert(dep.source_tree_path(), fetchgit_args);

            write_device_dir_file(device_dirs_path, &device_dirs)
                .map_err(|e| FetchDeviceDirsError::WriteFile(e))?;
        }

        if !branch_present_on_all_repos {
            *(device_dirs.get_mut(device_name).unwrap()) = None;
        }

        write_device_dir_file(device_dirs_path, &device_dirs)
            .map_err(|e| FetchDeviceDirsError::WriteFile(e))?;
    }

    Ok(device_dirs)
}

pub fn incrementally_fetch_vendor_dirs(devices: &HashMap<String, DeviceMetadata>, branch: &str, device_dirs: &HashMap<String, Option<DeviceDir>>, vendor_dirs_path: &str) -> Result<HashMap<String, Option<DeviceDir>>, FetchDeviceDirsError> {
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
