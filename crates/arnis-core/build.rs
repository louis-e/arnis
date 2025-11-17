#[cfg(feature = "gui")]
fn main() {
    println!("cargo:rerun-if-changed=tauri.conf.json");
    tauri_build::build();
}

#[cfg(not(feature = "gui"))]
fn main() {}
