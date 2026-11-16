use std::hash::{Hash, Hasher};

fn main() {
    // The window's HTML/CSS/JS are embedded when this crate compiles. Cargo only watches the `ui`
    // folder itself (its mtime doesn't change when a file inside is edited), and rerunning this
    // script alone doesn't recompile the crate. So watch each file and pass a hash of their
    // contents as an env var: a new value makes Cargo rebuild the crate with the new files.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    if let Ok(entries) = std::fs::read_dir("../../ui") {
        let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            println!("cargo:rerun-if-changed={}", path.display());
            path.hash(&mut hasher);
            if let Ok(bytes) = std::fs::read(&path) {
                bytes.hash(&mut hasher);
            }
        }
    }
    println!("cargo:rerun-if-changed=../../ui");
    println!("cargo:rustc-env=TETHER_UI_HASH={:x}", hasher.finish());
    tauri_build::build()
}
