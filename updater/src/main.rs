use std::collections::{HashMap, HashSet};
use std::fs;
use std::fs::File;
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

fn get_device_metadata_from_hudson(hudson_path: &str) -> HashMap<String, DeviceMetadata> {
    let build_targets = {
        let text_bytes = fs::read(format!("{}/lineage-build-targets", hudson_path)).unwrap();
        let text = std::str::from_utf8(&text_bytes).unwrap();
        let mut build_targets = vec![];
        for line in text.split("\n") {
            if line.starts_with("#") || line == "" {
                continue;
            }
            let fields: Vec<&str> = line.split(" ").collect();
            let device = fields.get(0).unwrap().to_string();
            let variant = fields.get(1).unwrap().to_string();
            let branch = fields.get(2).unwrap().to_string();

            build_targets.push((device, variant, branch));
        }
        
        build_targets
    };

    let reader = BufReader::new(File::open(format!("{}/updater/devices.json", hudson_path)).unwrap());
    let hudson_devices: Vec<HudsonDevice> = serde_json::from_reader(reader).unwrap();
    let reader = BufReader::new(File::open(format!("{}/updater/device_deps.json", hudson_path)).unwrap());
    let hudson_device_deps: HashMap<String, Vec<String>> = serde_json::from_reader(reader).unwrap();

    let mut device_metadata = HashMap::new();

    for (device, variant, branch) in build_targets {
        let hudson_device = hudson_devices.iter().filter(|x| x.model == device).next().unwrap();
        let hudson_deps = hudson_device_deps.get(&device).unwrap();
        device_metadata.insert(device, DeviceMetadata { 
            name: hudson_device.name.clone(),
            branch: branch,
            // We use the json parser for strings like `userdebug` by wrapping them in quotation
            // marks, like `"userdebug"`. This is a dirty hack and I need to figure out how to do
            // this properly at some point.
            variant: serde_json::from_str(&format!("\"{}\"", variant)).unwrap(),
            vendor: hudson_device.oem.to_lowercase(),
            deps: hudson_deps.iter().map(|x| Repository {
                remote: Remote::LineageOS,
                path: x.split("_").map(|x| x.to_string()).collect(),
            }).collect()
        });
    }

    device_metadata
}

fn fetch_device_metadata() -> HashMap<String, DeviceMetadata> {
    let prefetch_git_output = nix_prefetch_git_repo(&Repository {
        remote: Remote::LineageOS,
        path: vec!["hudson".to_string()],
    });

    get_device_metadata_from_hudson(&prefetch_git_output.path)
}

fn ls_remote(repo: &Repository, branch: &str) -> String {
    let url = repo.url();
    println!("ls-remote'ing {}", &url);
    let output = Command::new("git")
        .arg("ls-remote")
        .arg(&url)
        .arg(format!("refs/heads/{branch}"))
        .output()
        .unwrap()
        .stdout;
    std::str::from_utf8(output.split(|x| x == &b'\t').nth(0).unwrap()).unwrap().to_string()
}

fn nix_prefetch_git_repo(repo: &Repository) -> NixPrefetchGitOutput {
    let repo_url = repo.url();
    println!("Prefetching {}", &repo_url);
    let output = Command::new("nix-prefetch-git")
        .arg(&repo_url)
        .output()
        .unwrap();

    serde_json::from_slice(&output.stdout).unwrap()
}

// fn get_corresponding_vendor_repos(repo: &Vec<String>) -> Nix

fn main() {
    let args: Vec<String> = env::args().collect();
    let device_metadata_path = &args[1];

    let fetched_device_metadata = fetch_device_metadata();
    println!("{:?}", fetched_device_metadata);
    let file = File::create(device_metadata_path).unwrap();
    let writer = BufWriter::new(file);
    serde_json::to_writer(writer, &fetched_device_metadata);

    let reader = BufReader::new(File::open(device_metadata_path).unwrap());
    let devices: HashMap<String, DeviceMetadata> = serde_json::from_reader(reader).unwrap();

    // Read pre-existing device_dirs.json for incremental updates.
    let mut device_dir_entries: HashMap<String, Device> = {
        let reader = File::open(&args[2]).map(|f| BufReader::new(f));
        if let Ok(reader) = reader {
            if let Ok(entries) = serde_json::from_reader(reader) {
                entries
            } else {
                HashMap::new()
            }
        } else {
            HashMap::new()
        }
    };
    for (device_name, device_metadata) in devices.iter() {
        if !device_dir_entries.contains_key(device_name) {
            device_dir_entries.insert(device_name.clone(), Device {
                deps: HashMap::new(),
            });
        }
        let mut device = device_dir_entries.get_mut(device_name).unwrap();

        for dep in device_metadata.deps.iter() {
            let path = dep.source_tree_path();
            let is_up_to_date = if let Some(args) = device.deps.get(&path) {
                let current_rev = ls_remote(dep, &device_metadata.branch);
                current_rev == args.rev
            } else {
                false
            };

            if !is_up_to_date {
                let output = nix_prefetch_git_repo(dep);
                device.deps.insert(path, FetchgitArgs {
                    url: output.url,
                    rev: output.rev,
                    hash: output.hash,
                    fetch_lfs: output.fetch_lfs,
                    fetch_submodules: output.fetch_submodules,
                    deep_clone: output.deep_clone,
                    leave_dot_git: output.leave_dot_git,
                });
            }
        }
        let device_dirs_json = serde_json::to_string(&device_dir_entries).unwrap();
        let mut device_dirs_file = File::create(&args[2]).unwrap();
        device_dirs_file.write_all(device_dirs_json.as_bytes());
    }
}
