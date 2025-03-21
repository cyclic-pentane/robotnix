use std::io;
use std::process::Command;
use serde::{Serialize, Deserialize};
use serde_json;
use git2;

#[derive(Debug, Serialize, Deserialize)]
pub enum Variant {
    #[serde(rename = "userdebug")]
    Userdebug,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FetchgitArgs {
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

impl FetchgitArgs {
    // TODO In the future, we might consider only storing fields that actually are arguments
    // fetchgit and deriving the store path based on the hash. (for instance with tvix?)
    pub fn path(&self) -> String {
        self.path.clone()
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum Remote {
    LineageOS,
    TheMuppetsGitHub,
    TheMuppetsGitLab
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Repository {
    pub remote: Remote,
    pub path: Vec<String>
}

impl Remote {
    pub fn base_url(&self) -> String {
        match self {
            Remote::LineageOS => "https://github.com/LineageOS",
            Remote::TheMuppetsGitHub => "https://github.com/TheMuppets",
            Remote::TheMuppetsGitLab => "https://gitlab.com/TheMuppets"
        }.to_string()
    }
}

impl Repository {
    pub fn new_device_repo(vendor: &str, device: &str) -> Repository {
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

    pub fn url(&self) -> String {
        format!("{}/{}", &self.remote.base_url(), &self.path.join("_"))
    }

    // Path of the git repository within the AOSP source tree. For instance,
    // android_device_fairphone_FP4 has the source tree path device/fairphone/FP4
    pub fn source_tree_path(&self) -> String {
        match self.path.get(0).map(|x| x.as_str()) {
            Some("android") => &self.path[1..],
            Some("proprietary") => &self.path[1..],
            Some(_) => panic!("Not implemented yet"),
            None => panic!("Empty path")
        }.join("/")
    }
}

#[derive(Debug)]
pub enum GetRevOfBranchError {
    Libgit(git2::Error),
    BranchNotFound,
}

pub fn get_rev_of_branch(repo: &Repository, branch: &str) -> Result<String, GetRevOfBranchError> {
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
pub enum NixPrefetchGitError {
    GetRevOfBranch(GetRevOfBranchError),
    IOError(io::Error),
    Parser(serde_json::Error),
}

pub fn nix_prefetch_git_repo(repo: &Repository, branch: &str, prev: Option<FetchgitArgs>) -> Result<FetchgitArgs, NixPrefetchGitError> {
    let rev = get_rev_of_branch(repo, branch)
        .map_err(|e| NixPrefetchGitError::GetRevOfBranch(e))?;
    
    let fetch = if let Some(ref fetchgit_args) = prev {
        fetchgit_args.rev != rev
    } else {
        true
    };

    if fetch {
        let repo_url = repo.url();
        println!("Prefetching {}", &repo_url);
        let output = Command::new("nix-prefetch-git")
            .arg(&repo_url)
            .arg("--rev")
            .arg(&rev)
            .output()
            .map_err(|e| NixPrefetchGitError::IOError(e))?;

        Ok(serde_json::from_slice(&output.stdout).map_err(|e| NixPrefetchGitError::Parser(e))?)
    } else {
        Ok(prev.unwrap())
    }
}
