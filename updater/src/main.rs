mod base;
mod lineage;
mod repo_lockfile;

use clap::Parser;
use crate::lineage::{
    read_device_metadata,
    fetch_device_metadata,
};
use crate::repo_lockfile::{
    incrementally_fetch_projects,
};

#[derive(Debug, Parser)]
struct Args {
    #[arg(long)]
    branch: Option<String>,

    #[arg(long)]
    device_metadata_file: String,

    #[arg(long)]
    device_dirs_file: Option<String>,
    
    #[arg(long)]
    vendor_dirs_file: Option<String>,

    #[arg(long)]
    fetch_device_metadata: bool,

    #[arg(long)]
    fetch_device_dirs: bool,

    #[arg(long)]
    fetch_vendor_dirs: bool,
}

fn main() {
    let args = Args::parse();

    if args.fetch_device_metadata {
        fetch_device_metadata(
            &args.device_metadata_file,
        ).unwrap();
    }

    if args.fetch_device_dirs {
        let branch = &args.branch
            .expect("You need to specify the branch to fetch device dirs for with --branch");
        let device_dirs_file = &args.device_dirs_file
            .expect("You need to specify the path to write the device dir file to with --device-dir-file");
        let devices = read_device_metadata(&args.device_metadata_file).unwrap();
        let mut device_dirs = vec![];
        let mut device_names: Vec<String> = devices.keys().map(|x| x.to_string()).collect();
        device_names.sort();
        for device_name in device_names {
            for device_dir in devices[&device_name].deps.iter() {
                if !device_dirs.contains(device_dir) {
                    device_dirs.push(device_dir.clone());
                }
            }
        }

        incrementally_fetch_projects(&device_dirs_file, &device_dirs, &branch).unwrap();
    };
}
