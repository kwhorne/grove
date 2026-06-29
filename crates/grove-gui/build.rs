fn main() {
    // Recompile (and re-embed the frontend) when the config or built assets
    // change — otherwise Cargo's incremental cache can keep a stale bundle.
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=ui/dist");
    println!("cargo:rerun-if-changed=ui/dist/index.html");
    tauri_build::build();
}
