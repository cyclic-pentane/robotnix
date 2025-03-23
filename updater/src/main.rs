mod base;
mod lineage;
mod repo_manifest;
mod repo_lockfile;

use clap::{Parser, Subcommand};
use crate::base::Repository;
use crate::lineage::{
    read_device_metadata,
    fetch_device_metadata,
};
use crate::repo_lockfile::{
    incrementally_fetch_projects,
};

#[derive(Debug, Parser)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    FetchRepoMetadata {
        #[arg(name = "branch", short, long)]
        branches: Vec<String>,

        repo_metadata_file: String,
    },
    FetchDeviceMetadata {
        #[arg(name = "branch", short, long)]
        branch: String,
        device_metadata_file: String,
    },
    FetchDeviceDirs {
        #[arg(long)]
        device_metadata_file: String,

        #[arg(short, long)]
        branch: String,

        device_dirs_file: String,
    }
}

fn main() {
    let args = Args::parse();


    match args.command.expect("You need to specify a command.") {
        Command::FetchRepoMetadata { branches, repo_metadata_file } => {
            fetch_git_repo_metadata(
                &repo_metadata_file,
                &Repository {
                    url: "https://github.com/LineageOS/android".to_string(),
                },
                &branches
            ).unwrap();
        },
        Command::FetchDeviceMetadata { branch, device_metadata_file } => {
            fetch_device_metadata(
                &device_metadata_file,
                &branch,
            ).unwrap();
        },
    }
}
