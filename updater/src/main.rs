use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::env;
use std::io::BufReader;
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
    deps: Vec<Vec<String>>
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

enum Remote {
    LineageOS,
    TheMuppetsGitHub,
    TheMuppetsGitLab
}

struct Repository {
    remote: Remote,
    path: Vec<String>
}

impl Remote {
    fn base_url(&self) -> String {
        match self {
            Repository::LineageOS => "https://github.com/LineageOS/",
            Repository::TheMuppetsGitHub => "https://github.com/TheMuppets",
            Repository::TheMuppetsGitLab => "https://gitlab.com/TheMuppets"
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
                vendor.clone(),
                device.clone()
            ]
        }
    }

    fn url(&self) -> String {
        format!("{}{}", &self.repo.base_url(), &self.path.join("_")
    }

    fn source_tree_path(&self) -> String {
        match self.path.get(0) {
            Some("android") => &self.path[1..],
            Some("proprietary") => &self.path[1..],
            Some(xs) => xs,
            None => &vec![]
        }.join("/")
    }
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

fn nix_prefetch_git_repo(repo: &Vec<String>) -> NixPrefetchGitOutput {
    let repo_url = get_repo_url(repo);
    println!("Prefetching {}", &repo_url);
    let output = Command::new("nix-prefetch-git")
        .arg(&repo_url)
        .output()
        .unwrap();

    serde_json::from_slice(&output.stdout).unwrap()
}

fn get_corresponding_vendor_repos(repo: &Vec<String>) -> Nix

fn main() {
    let args: Vec<String> = env::args().collect();
    let device_metadata_path = &args[1];

    let reader = BufReader::new(File::open(device_metadata_path).unwrap());
    let devices: HashMap<String, DeviceMetadata> = serde_json::from_reader(reader).unwrap();

    // Read pre-existing device_dirs.json for incremental updates.
    let reader = File::open(&args[2]).map(|f| BufReader::new(f));
    let mut device_dir_entries: HashMap<String, Device> = reader.map(|r| serde_json::from_reader(r)).unwrap_or(HashMap::new());
    for (device_name, device_metadata) in devices.iter() {
        if !device_dir_entries.contains_key(device_name) {
            device_dir_entries.insert(device_name.clone(), Device {
                deps: HashMap::new(),
            });
        }
        let mut device = device_dir_entries.get_mut(device_name).unwrap();

        for dep in device_metadata.deps.iter() {
            let path = dep[1..].join("/");
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
