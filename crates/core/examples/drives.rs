//! Prints what Unlost sees about each drive, as JSON. Needs no administrator rights:
//! `cargo run -p unlost-core --example drives`

fn main() {
    let drives = unlost_core::disks::drives();
    println!(
        "{}",
        serde_json::to_string_pretty(&drives).unwrap_or_default()
    );
    for d in &drives {
        let set = unlost_core::guard::disks_of_path(std::path::Path::new(&d.root));
        println!("{} -> {:?}", d.root, set.map(|s| s.disks));
    }
    println!("media devices: {:?}", unlost_core::disks::media_devices());
}
