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
    ReadManifest(ReadManifestError),
    Utf8(std::str::Utf8Error),
    InvalidLineageBuildTargets,
    Parser(serde_json::Error),
    ModelNotFoundInUpdaterDir(String),
    UnknownBranch(String),
}

fn fetch_lineage_manifests_for_branches(branches: &[String]) -> Result<HashMap<String, GitRepoManifest>, FetchDeviceMetadataError> {
    let mut lineage_manifests = HashMap::new();
    for branch in branches.iter() {
        println!("Fetching LineageOS manifest repo (branch {})", &branch);
        let fetchgit_args = nix_prefetch_git_repo(
            &Repository {
                url: "https://github.com/LineageOS/android".to_string(),
            }, &format!("refs/heads/{branch}"), None).map_err(|e| FetchDeviceMetadataError::PrefetchGit(e))?;

        let manifest = GitRepoManifest::read_and_flatten(
            &Path::new(&fetchgit_args.path()),
            Path::new("default.xml")
        ).map_err(|e| FetchDeviceMetadataError::ReadManifest(e))?;

        lineage_manifests.insert(branch.to_string(), manifest);
    }

    Ok(lineage_manifests)
}

fn fetch_muppets_manifests_for_branches(branches: &[String]) -> Result<HashMap<String, GitRepoManifest>, FetchDeviceMetadataError> {
    let mut muppets_manifests = HashMap::new();
    for branch in branches.iter() {
        if !muppets_manifests.contains_key(branch) {
            println!("Fetching TheMuppets manifest (branch {branch})...");
            let muppets = nix_prefetch_git_repo(&Repository {
                url: "https://github.com/TheMuppets/manifests".to_string(),
            }, &format!("refs/heads/{branch}"), None).map_err(|e| FetchDeviceMetadataError::PrefetchGit(e))?;

            let muppets_manifest = GitRepoManifest::read(Path::new(&muppets.path()), Path::new("muppets.xml"))
                .map_err(|e| FetchDeviceMetadataError::ReadManifest(e))?;
            muppets_manifests.insert(branch.clone(), muppets_manifest);
        }
    }

    Ok(muppets_manifests)
}

#[derive(Debug, Serialize, Deserialize)]
struct LineageDependency {
    repository: String,
    target_path: String,

    #[serde(default)]
    branch: Option<String>,

    #[serde(default)]
    remote: Option<String>,
}

fn parse_build_targets(hudson_path: &str) -> Result<Vec<(String, String, String)>, FetchDeviceMetadataError> {
    let text_bytes = fs::read(format!("{}/lineage-build-targets", &hudson_path))
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
    
    Ok(build_targets)
}

fn fetch_lineage_dependencies(vendor: &str, device_name: &str, branch: &str) -> Result<Vec<LineageDependency>, FetchDeviceMetadataError> {
    // Currently, we need to infer the vendor code from the human-readable vendor name (e.g.
    // `bananapi` from "Banana Pi". It would be cool to programmatically pull this from somewhere
    // though.
    // TODO softcode these overrides (maybe a JSON config file or something)
    let mut vendor_name = vendor.to_lowercase().replace(" ", "");
    if device_name == "deadpool" || device_name == "wade" || device_name == "dopinder" {
        vendor_name = "askey".to_string();
    } else if device_name == "deb" || device_name == "debx" {
        vendor_name = "asus".to_string();
    } else if device_name == "ingot" {
        vendor_name = "osom".to_string();
    }

    if vendor_name == "lg" {
        vendor_name = "lge".to_string();
    } else if vendor_name == "f(x)tec" {
        vendor_name = "fxtec".to_string();
    }

    let repo_name = format!("android_device_{vendor_name}_{device_name}");
    println!("Fetching device repo {repo_name} (branch {branch})...");
    let device_repo = nix_prefetch_git_repo(&Repository {
        url: format!("https://github.com/LineageOS/{repo_name}"),
    }, &format!("refs/heads/{branch}"), None).map_err(|e| FetchDeviceMetadataError::PrefetchGit(e))?;

    let json_bytes = fs::read(format!("{}/lineage.dependencies", &device_repo.path()))
        .map_err(|e| FetchDeviceMetadataError::FileRead(e))?;
    let json = std::str::from_utf8(&json_bytes)
        .map_err(|e| FetchDeviceMetadataError::Utf8(e))?;
    let mut deps: Vec<LineageDependency> = serde_json::from_str(&json)
        .map_err(|e| FetchDeviceMetadataError::Parser(e))?;

    deps.insert(0, LineageDependency {
        repository: repo_name,
        target_path: format!("device/{vendor_name}/{device_name}"),
        branch: Some(branch.to_string()),
        remote: Some("github".to_string()),
    });

    Ok(deps)
}

pub fn fetch_device_metadata(device_metadata_path: &str) -> Result<HashMap<String, DeviceMetadata>, FetchDeviceMetadataError> {
    println!("Fetching LineageOS hudson...");
    let hudson = nix_prefetch_git_repo(&Repository {
        url: "https://github.com/LineageOS/hudson".to_string(),
    }, &"refs/heads/main", None).map_err(|e| FetchDeviceMetadataError::PrefetchGit(e))?;

    let build_targets = parse_build_targets(&hudson.path())?;
    let mut all_branches = vec![];
    for (_, _, branch) in build_targets.iter() {
        if !all_branches.contains(branch) {
            all_branches.push(branch.to_string())
        }
    }
    let lineage_manifests = fetch_lineage_manifests_for_branches(all_branches.as_ref())?;
    let muppets_manifests = fetch_muppets_manifests_for_branches(all_branches.as_ref())?;

    let reader = BufReader::new(File::open(format!("{}/updater/devices.json", &hudson.path()))
        .map_err(|e| FetchDeviceMetadataError::FileRead(e))?);
    let hudson_devices: Vec<HudsonDevice> = serde_json::from_reader(reader)
        .map_err(|e| FetchDeviceMetadataError::Parser(e))?;

    let mut device_metadata = HashMap::new();

    // TODO make this multi-branch as soon as I find out where to get the information about the
    // device's supported branches from.
    for (device, variant, branch) in build_targets {
        let hudson_device = hudson_devices.iter().filter(|x| x.model == device).next().ok_or(FetchDeviceMetadataError::ModelNotFoundInUpdaterDir(device.clone()))?;
        let manifest = lineage_manifests.get(&branch).unwrap();
        let real_branch = {
            // TODO currently we need to infer this, but there should be a better way.
            // TODO softcode this
            if branch == "lineage-21.0" {
                "lineage-21"
            } else {
                branch.as_ref()
            }
        };
        let deps = fetch_lineage_dependencies(&hudson_device.oem, &device, &real_branch)?;

        let mut projects = vec![];
        for dep in deps {
            let custom_ref = dep.branch.map(|x| format!("refs/heads/{x}"));
            let (remote_url, git_ref) = manifest.get_url_and_ref(
                &dep.remote,
                &custom_ref, 
                &"https://github.com/LineageOS/android"
            ).map_err(|e| FetchDeviceMetadataError::ReadManifest(e))?;

            // TODO softcode this too
            let git_ref = if git_ref == "refs/heads/lineage-21.0" {
                "refs/heads/lineage-21".to_string()
            } else {
                git_ref
            };

            let remote_url = if remote_url == "https://github.com" {
                "https://github.com/LineageOS".to_string()
            } else {
                remote_url
            };

            let project = RepoProject {
                nonfree: false,
                path: dep.target_path,
                branch_settings: {
                    let mut branch_settings = HashMap::new();
                    branch_settings.insert(branch.clone(), RepoProjectBranchSettings {
                        repo: Repository {
                            url: format!("{}/{}", &remote_url, &dep.repository)
                        },
                        git_ref: git_ref,
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
                real_branch,
        ));

        device_metadata.insert(device.clone(), DeviceMetadata { 
            name: hudson_device.name.clone(),
            branch: branch.clone(),
            // TODO We use the json parser for strings like `userdebug` by wrapping them in quotation
            // marks, like `"userdebug"`. This is a dirty hack and I need to figure out how to do
            // this properly at some point.
            variant: serde_json::from_str(&format!("\"{}\"", variant)).map_err(|e| FetchDeviceMetadataError::Parser(e))?,
            vendor: hudson_device.oem.clone(),
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
