use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let json_path = manifest_dir
        .parent()
        .expect("workspace root")
        .join("ledface_layout/test/test.json");

    println!("cargo:rerun-if-changed={}", json_path.display());

    let data = fs::read(&json_path).expect("failed to read layout JSON");
    let layout: ledface_layout::Layout<64, 16, 64> =
        ledface_layout::Layout::from_json(&data).expect("invalid layout JSON");
    let bytes = postcard::to_allocvec(&layout).expect("failed to serialize layout");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let out_file = out_dir.join("watchface_layout.bin");
    fs::write(out_file, &bytes).expect("failed to write compiled layout");
}
