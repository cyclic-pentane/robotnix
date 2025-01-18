use std::collections::{HashMap, HashSet};
use std::fs::File;
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
    fetchLFS: bool,
    fetchSubmodules: bool,
    deepClone: bool,
    leaveDotGit: bool,
}

fn get_device_repo(vendor: &str, device: &str) -> Vec<String> {
    vec!["android", "device", vendor, device].iter().map(|x| x.to_string()).collect()
}

fn nix_prefetch_git_repo(repo: &Vec<String>) -> NixPrefetchGitOutput {
    let repo_url = format!("https://github.com/LineageOS/{}", repo.join("_"));
    let output = Command::new("nix-prefetch-git")
        .arg(&repo_url)
        .output()
        .unwrap();

    serde_json::from_slice(&output.stdout).unwrap()
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let device_metadata_path = &args[1];

    let file = File::open(device_metadata_path).unwrap();
    let reader = BufReader::new(file);
    let devices: HashMap<String, DeviceMetadata> = serde_json::from_reader(reader).unwrap();

    let mut repos_to_fetch: HashSet<Vec<String>> = HashSet::new();
    for (device_name, device_metadata) in devices.iter() {
        repos_to_fetch.insert(get_device_repo(&device_metadata.vendor, &device_name));

        for dep in device_metadata.deps.iter() {
            repos_to_fetch.insert(dep.clone());
        }
    }

    let mut prefetch_outputs: Vec<NixPrefetchGitOutput> = Vec::new();
    for repo in repos_to_fetch.iter() {
        let output = nix_prefetch_git_repo(repo);
        println!("{:?}", &output);
        prefetch_outputs.push(output);
    }
}
