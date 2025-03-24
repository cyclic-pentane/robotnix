use std::vec::Vec;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::io::Write;
use std::str;
use std::path::{Path, PathBuf};
use serde::{Serialize, Deserialize};
use quick_xml;
use atomic_write_file::AtomicWriteFile;

use crate::base::{
    Repository,
    RepoProject,
    RepoProjectBranchSettings,
    nix_prefetch_git_repo,
    NixPrefetchGitError
};

#[derive(Debug, Serialize, Deserialize)]
pub struct GitRepoRemote {
    #[serde(rename = "@name")]
    name: String,

    #[serde(rename = "@fetch")]
    fetch: String,

    #[serde(rename = "@review")]
    review: Option<String>,

    #[serde(rename = "@revision")]
    default_ref: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GitRepoDefaultRemote {
    #[serde(rename = "@remote")]
    remote: String,

    #[serde(rename = "@revision")]
    default_ref: Option<String>,

    #[serde(rename = "@sync-c")]
    sync_c: String,

    #[serde(rename = "@sync-j")]
    sync_j: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct GitRepoLinkfile {
    #[serde(rename = "@src")]
    src: String,

    #[serde(rename = "@dest")]
    dest: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct GitRepoCopyfile {
    #[serde(rename = "@src")]
    src: String,

    #[serde(rename = "@dest")]
    dest: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct GitRepoProject {
    #[serde(rename = "@path")]
    path: String,

    #[serde(rename = "@name")]
    repo_name: String,

    #[serde(rename = "@groups")]
    groups: Option<String>,

    #[serde(rename = "@remote")]
    remote: Option<String>,

    #[serde(rename = "@revision")]
    git_ref: Option<String>,

    #[serde(rename = "linkfile", default)]
    linkfiles: Vec<GitRepoLinkfile>,

    #[serde(rename = "copyfile", default)]
    copyfiles: Vec<GitRepoCopyfile>,
}

// TODO use Path and PathBuf everywhere where they're applicable
#[derive(Debug, Serialize, Deserialize)]
struct GitRepoInclude {
    #[serde(rename = "@name")]
    name: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename = "manifest")]
struct GitRepoManifest {
    #[serde(default, rename = "remote")]
    remotes: Vec<GitRepoRemote>,

    #[serde(rename = "default")]
    default_remote: Option<GitRepoDefaultRemote>,

    #[serde(rename = "project")]
    projects: Vec<GitRepoProject>,

    #[serde(rename = "include")]
    includes: Vec<GitRepoInclude>,
}

#[derive(Debug)]
pub enum ReadManifestError {
    FileRead(io::Error),
    Utf8(str::Utf8Error),
    Parser(quick_xml::errors::serialize::DeError),
    MoreThanOneDefaultRemote,
}

fn read_manifest_file(manifest_path: &Path, filename: &Path) -> Result<GitRepoManifest, ReadManifestError> {
    let manifest_xml_bytes = fs::read(manifest_path.join(filename))
        .map_err(|e| ReadManifestError::FileRead(e))?;

    let manifest_xml = str::from_utf8(&manifest_xml_bytes)
        .map_err(|e| ReadManifestError::Utf8(e))?;

    let manifest: GitRepoManifest = quick_xml::de::from_str(&manifest_xml)
        .map_err(|e| ReadManifestError::Parser(e))?;

    Ok(manifest)
}


fn read_and_flatten_manifest(manifest_path: &Path, filename: &Path) -> Result<GitRepoManifest, ReadManifestError> {
    let mut manifest = read_manifest_file(manifest_path, filename)?;

    for include in manifest.includes.iter() {
        let mut submanifest = read_and_flatten_manifest(manifest_path, &include.name)?;

        manifest.remotes.append(&mut submanifest.remotes);
        manifest.projects.append(&mut submanifest.projects);
        
        if let Some(default_remote) = submanifest.default_remote {
            if let None = manifest.default_remote {
                manifest.default_remote = Some(default_remote);
            } else {
                return Err(ReadManifestError::MoreThanOneDefaultRemote);
            }
        }
    }

    manifest.includes = vec![];
    Ok(manifest)
}

struct RemoteSpec {
    url: String,
    default_ref: Option<String>,
}

fn get_remote_specs_from_manifest(manifest: &GitRepoManifest, root_url: &str) -> HashMap<String, RemoteSpec> {
    let mut remote_specs = HashMap::new();
    for remote in manifest.remotes.iter() {
        let is_default_remote = manifest.default_remote
            .as_ref()
            .map(|x| x.remote == remote.name)
            .unwrap_or(false);
        let default_ref = if is_default_remote {
            manifest.default_remote
                .as_ref()
                .unwrap()
                .default_ref
                .as_ref()
                .or(remote.default_ref.as_ref())
        } else {
            remote.default_ref.as_ref()
        };
        remote_specs.insert(remote.name.clone(), RemoteSpec {
            url: {
                if remote.fetch != ".." {
                    remote.fetch.clone()
                } else {
                    let url_parts: Vec<String> = root_url
                        .split("/")
                        .map(|x| x.to_string())
                        .collect();
                    url_parts[0..url_parts.len()-2].join("/")
                }
            },
            default_ref: default_ref.map(|x| x.to_string()),
        });
    }

    remote_specs
}

fn get_projects_from_manifest(manifest: &GitRepoManifest, projects: &mut HashMap<String, RepoProject>, root_url: &str, branch: &str) -> Result<(), FetchGitRepoMetadataError> {
    let remote_specs = get_remote_specs_from_manifest(manifest, root_url);

    for project in manifest.projects.iter() {
        let remote_name = project.remote.as_ref().unwrap_or(
            &manifest.default_remote.as_ref().ok_or(FetchGitRepoMetadataError::MissingDefaultRemote)?.remote
        ).clone();
        let remote = remote_specs
            .get(&remote_name)
            .ok_or(FetchGitRepoMetadataError::UnknownRemote(remote_name))?;
        let remote_url_trunc = if remote.url.ends_with("/") {
            &remote.url[0..remote.url.len()-1]
        } else {
            &remote.url
        };
        let project_url = format!("{}/{}", remote_url_trunc, &project.repo_name);

        if !projects.contains_key(&project.path) {
            projects.insert(project.path.clone(), RepoProject {
                path: project.path.clone(),
                nonfree: false,
                branch_settings: HashMap::new(),
            });
        }

        let branch_settings = &mut projects
            .get_mut(&project.path.clone())
            .unwrap()
            .branch_settings;
        branch_settings.insert(branch.to_string(), RepoProjectBranchSettings {
            repo: Repository {
                url: project_url,
            },
            copyfiles: {
                let mut files = HashMap::new();
                for c in project.copyfiles.iter() {
                    files.insert(c.dest.clone(), c.src.clone());
                }
                files
            },
            linkfiles: {
                let mut files = HashMap::new();
                for l in project.linkfiles.iter() {
                    files.insert(l.dest.clone(), l.src.clone());
                }
                files
            },
            git_ref: project.git_ref.as_ref()
                .or(remote.default_ref.as_ref())
                .cloned()
                .ok_or(FetchGitRepoMetadataError::MissingDefaultRef)?,
        });
    }

    Ok(())
}

#[derive(Debug)]
pub enum FetchGitRepoMetadataError {
    PrefetchGit(NixPrefetchGitError),
    ReadManifest(ReadManifestError),
    UnknownRemote(String),
    MissingDefaultRemote,
    MissingDefaultRef,
    FileWrite(io::Error),
    Parser(serde_json::Error),
}

pub fn fetch_git_repo_metadata(filename: &str, manifest_repo: &Repository, branches: &[String]) -> Result<Vec<RepoProject>, FetchGitRepoMetadataError> {
    let mut projects: HashMap<String, RepoProject> = HashMap::new();

    for branch in branches.iter() {
        println!("Fetching manifest repo {} (branch {})", &manifest_repo.url, &branch);
        let fetchgit_args = nix_prefetch_git_repo(manifest_repo, &format!("refs/heads/{branch}"), None)
            .map_err(|e| FetchGitRepoMetadataError::PrefetchGit(e))?;

        let manifest = read_manifest_file(
            &Path::new(&fetchgit_args.path()),
            Path::new("default.xml")
        ).map_err(|e| FetchGitRepoMetadataError::ReadManifest(e))?;

        get_projects_from_manifest(&manifest, &mut projects, &manifest_repo.url, branch)?;
    }

    let mut projects: Vec<RepoProject> = projects.values().cloned().collect();
    projects.sort_by_key(|p| p.path.clone());

    let projects_json = serde_json::to_string_pretty(&projects)
        .map_err(|e| FetchGitRepoMetadataError::Parser(e))?;
    let mut file = AtomicWriteFile::options().open(filename)
        .map_err(|e| FetchGitRepoMetadataError::FileWrite(e))?;
    file.write(projects_json.as_bytes())
        .map_err(|e| FetchGitRepoMetadataError::FileWrite(e))?;
    file.commit()
        .map_err(|e| FetchGitRepoMetadataError::FileWrite(e))?;

    Ok(projects)
}
