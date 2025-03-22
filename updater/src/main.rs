mod base;
mod lineage;

use clap::Parser;
use lineage::{
    read_device_metadata,
    fetch_device_metadata,
    incrementally_fetch_device_dirs,
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
        let devices = read_device_metadata(&args.device_metadata_file).unwrap();
        incrementally_fetch_device_dirs(
            &devices,
            &args.branch.expect(&"You need to specify the branch to fetch device dirs for with --branch"),
            args.device_dirs_file.as_ref().expect(&"You need to set --device-dirs-file to specify the location to store the device dirs JSON to")
        ).unwrap();
    };
}
