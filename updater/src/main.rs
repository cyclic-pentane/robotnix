mod base;
mod lineage;

use clap::Parser;
use lineage::{
    fetch_device_metadata_to,
    read_device_metadata,
    read_device_dir_file,
    incrementally_fetch_device_dirs,
    incrementally_fetch_vendor_dirs,
};

#[derive(Debug, Parser)]
struct Args {
    #[arg(long)]
    branch: String,

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
        fetch_device_metadata_to(
            &args.device_metadata_file,
            &args.branch,
        ).unwrap()
    }

    if args.fetch_device_dirs {
        let devices = read_device_metadata(&args.device_metadata_file).unwrap();
        incrementally_fetch_device_dirs(
            &devices,
            &args.branch,
            args.device_dirs_file.as_ref().expect(&"You need to set --device-dirs-file to specify the location to store the device dirs JSON to")
        ).unwrap();
    };

    if args.fetch_vendor_dirs {
        let devices = read_device_metadata(&args.device_metadata_file).unwrap();
        let device_dirs = read_device_dir_file(
            args.device_dirs_file.as_ref().expect(&"You need to set --device-dirs-file to fetch the corresponding vendor dirs")
        ).unwrap();
        incrementally_fetch_vendor_dirs(
            &devices,
            &args.branch,
            &device_dirs,
            args.vendor_dirs_file.as_ref().expect(&"You need set --vendor-dirs-file to specify the location to store the vendor dirs JSON to")
        );
    }
}
