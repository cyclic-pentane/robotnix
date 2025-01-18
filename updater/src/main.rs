use std::collections::HashMap;
use std::fs::File;
use std::env;
use std::io::BufReader;
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
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let device_metadata_path = &args[1];

    let file = File::open(device_metadata_path).unwrap();
    let reader = BufReader::new(file);
    let devices: HashMap<String, DeviceMetadata> = serde_json::from_reader(reader).unwrap();

    println!("{devices:?}")
}
