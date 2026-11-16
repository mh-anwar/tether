fn main() {
    // The window's HTML/CSS/JS are embedded at compile time. Cargo only watches the `ui` folder
    // itself, whose mtime doesn't change when a file inside is edited, so list each file.
    if let Ok(entries) = std::fs::read_dir("../../ui") {
        for entry in entries.flatten() {
            println!("cargo:rerun-if-changed={}", entry.path().display());
        }
    }
    println!("cargo:rerun-if-changed=../../ui");
    tauri_build::build()
}
