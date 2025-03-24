use std::collections::HashMap;
use std::str;
use std::fs;
use std::fs::File;
use std::path::Path;
use atomic_write_file::AtomicWriteFile;
use std::io;
use std::io::Write;
use std::io::BufReader;
use serde::{Serialize, Deserialize};
use serde_json;
use quick_xml;

use crate::base::{
    Variant,
    Repository,
    RepoProject,
    RepoProjectBranchSettings,
    NixPrefetchGitError,
    nix_prefetch_git_repo,
    FetchgitArgs,
};

use crate::repo_manifest::{
    GitRepoManifest,
    read_manifest_file,
    ReadManifestError,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct DeviceMetadata {
    pub branch: String,
    pub vendor: String,
    pub name: String,
    pub variant: Variant,
    pub deps: Vec<RepoProject>,
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

fn get_proprietary_repos_for_device(muppets_manifests: &GitRepoManifest, device: &str, branch: &str) -> Vec<RepoProject> {
    let mut repos = vec![];
    for entry in muppets_manifests.projects.iter() {
        let mut found = false;
        if let Some(groups) = &entry.groups {
            for m_group in groups.split(",") {
                if m_group == format!("muppets_{device}") {
                    found = true;
                    break;
                }
            }
            if found {
                let mut repo_name = "proprietary".to_string();
                for c in entry.path.split("/") {
                    repo_name.push('_');
                    repo_name.push_str(c);
                }
                repos.push(RepoProject {
                    path: entry.path.clone(),
                    nonfree: true,
                    branch_settings: {
                        let mut branch_settings = HashMap::new();
                        branch_settings.insert(branch.to_string(), RepoProjectBranchSettings {
                            repo: Repository {
                                url: format!("https://github.com/TheMuppets/{repo_name}"),
                            },
                            git_ref: format!("refs/heads/{branch}"),
                            linkfiles: HashMap::new(),
                            copyfiles: HashMap::new(),
                        });
                        branch_settings
                    },
                });
            }
        }
    }

    repos
}

#[derive(Debug)]
pub enum FetchDeviceMetadataError {
    PrefetchGit(NixPrefetchGitError),
    FileRead(io::Error),
    FileWrite(io::Error),
    ReadMuppetsManifest(ReadManifestError),
    Utf8(std::str::Utf8Error),
    InvalidLineageBuildTargets,
    Parser(serde_json::Error),
    ModelNotFoundInUpdaterDir(String),
    UnknownBranch(String),
}

fn fetch_muppets_manifests_for_branches(branches: &[String]) -> Result<HashMap<String, GitRepoManifest>, FetchDeviceMetadataError> {
    let mut muppets_manifests = HashMap::new();
    for branch in branches.iter() {
        if !muppets_manifests.contains_key(branch) {
            println!("Fetching TheMuppets manifest (branch {branch})...");
            let muppets = nix_prefetch_git_repo(&Repository {
                url: "https://github.com/TheMuppets/manifests".to_string(),
            }, &format!("refs/heads/{branch}"), None).map_err(|e| FetchDeviceMetadataError::PrefetchGit(e))?;

            let muppets_manifest = read_manifest_file(Path::new(&muppets.path()), Path::new("muppets.xml"))
                .map_err(|e| FetchDeviceMetadataError::ReadMuppetsManifest(e))?;
            muppets_manifests.insert(branch.clone(), muppets_manifest);
        }
    }

    Ok(muppets_manifests)
}

pub fn fetch_device_metadata(device_metadata_path: &str) -> Result<HashMap<String, DeviceMetadata>, FetchDeviceMetadataError> {
    println!("Fetching LineageOS hudson...");
    let hudson = nix_prefetch_git_repo(&Repository {
        url: "https://github.com/LineageOS/hudson".to_string(),
    }, &"refs/heads/main", None).map_err(|e| FetchDeviceMetadataError::PrefetchGit(e))?;

    let build_targets = {
        let text_bytes = fs::read(format!("{}/lineage-build-targets", &hudson.path()))
            .map_err(|e| FetchDeviceMetadataError::FileRead(e))?;
        let text = std::str::from_utf8(&text_bytes)
            .map_err(|e| FetchDeviceMetadataError::Utf8(e))?;
        let mut build_targets = vec![];
        for line in text.split("\n") {
            if line.starts_with("#") || line == "" {
                continue;
            }
            let fields: Vec<&str> = line.split(" ").collect();
            let device  = fields.get(0).ok_or(FetchDeviceMetadataError::InvalidLineageBuildTargets)?.to_string();
            let variant = fields.get(1).ok_or(FetchDeviceMetadataError::InvalidLineageBuildTargets)?.to_string();
            let branch  = fields.get(2).ok_or(FetchDeviceMetadataError::InvalidLineageBuildTargets)?.to_string();

            build_targets.push((device, variant, branch));
        }
        
        build_targets
    };

    let branches: Vec<String> = build_targets.iter().map(|x| x.2.clone()).collect();
    let muppets_manifests = fetch_muppets_manifests_for_branches(branches.as_ref())?;

    let reader = BufReader::new(File::open(format!("{}/updater/devices.json", &hudson.path()))
        .map_err(|e| FetchDeviceMetadataError::FileRead(e))?);
    let hudson_devices: Vec<HudsonDevice> = serde_json::from_reader(reader)
        .map_err(|e| FetchDeviceMetadataError::Parser(e))?;
    let reader = BufReader::new(File::open(format!("{}/updater/device_deps.json", &hudson.path()))
        .map_err(|e| FetchDeviceMetadataError::FileRead(e))?);
    let hudson_device_deps: HashMap<String, Vec<String>> = serde_json::from_reader(reader)
        .map_err(|e| FetchDeviceMetadataError::Parser(e))?;

    let mut device_metadata = HashMap::new();

    for (device, variant, branch) in build_targets {
        let hudson_device = hudson_devices.iter().filter(|x| x.model == device).next().ok_or(FetchDeviceMetadataError::ModelNotFoundInUpdaterDir(device.clone()))?;
        let mut hudson_deps = hudson_device_deps.get(&device).ok_or(FetchDeviceMetadataError::ModelNotFoundInUpdaterDir(device.clone()))?.clone();
        hudson_deps.sort();

        let mut projects = vec![];
        for repo_name in hudson_deps {
            let path = repo_name
                .split("_")
                .skip(1)
                .collect::<Vec<&str>>()
                .as_slice()
                .join("/");

            let project = RepoProject {
                nonfree: false,
                path: path,
                branch_settings: {
                    let mut branch_settings = HashMap::new();
                    branch_settings.insert(branch.clone(), RepoProjectBranchSettings {
                        repo: Repository {
                            url: format!("https://github.com/LineageOS/{repo_name}")
                        },
                        git_ref: format!("refs/heads/{branch}"),
                        copyfiles: HashMap::new(),
                        linkfiles: HashMap::new(),
                    });
                    branch_settings
                },
            };
            projects.push(project);
        }

        projects.append(&mut get_proprietary_repos_for_device(
                muppets_manifests.get(&branch).unwrap(),
                &device,
                &branch,
        ));

        device_metadata.insert(device.clone(), DeviceMetadata { 
            name: hudson_device.name.clone(),
            branch: branch.clone(),
            // TODO We use the json parser for strings like `userdebug` by wrapping them in quotation
            // marks, like `"userdebug"`. This is a dirty hack and I need to figure out how to do
            // this properly at some point.
            variant: serde_json::from_str(&format!("\"{}\"", variant)).map_err(|e| FetchDeviceMetadataError::Parser(e))?,
            vendor: hudson_device.oem.to_lowercase(),
            deps: projects,
        });
    }

    let mut file = AtomicWriteFile::options()
        .open(device_metadata_path)
        .map_err(|e| FetchDeviceMetadataError::FileWrite(e))?;
    let buf = serde_json::to_string_pretty(&device_metadata)
        .map_err(|e| FetchDeviceMetadataError::Parser(e))?;

    file.write(buf.as_bytes()).map_err(|e| FetchDeviceMetadataError::FileWrite(e))?;
    file.commit().map_err(|e| FetchDeviceMetadataError::FileWrite(e))?;

    Ok(device_metadata)
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
