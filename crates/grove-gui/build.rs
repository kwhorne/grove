use std::path::Path;

fn main() {
    // The `grove` CLI is bundled as a Tauri sidecar (binaries/grove-<triple>) so
    // the desktop app can start the daemon. Release CI stages the real binary
    // before building; for local/dev builds we drop a placeholder so the build
    // doesn't fail (devs run `grove` from PATH, not the bundled copy).
    if let Ok(target) = std::env::var("TARGET") {
        let ext = if target.contains("windows") {
            ".exe"
        } else {
            ""
        };
        let path = format!("binaries/grove-{target}{ext}");
        if !Path::new(&path).exists() {
            std::fs::create_dir_all("binaries").ok();
            std::fs::write(&path, b"").ok();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
            }
        }
    }

    // Recompile (and re-embed the frontend) when the config or built assets
    // change — otherwise Cargo's incremental cache can keep a stale bundle.
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=ui/dist");
    println!("cargo:rerun-if-changed=ui/dist/index.html");
    tauri_build::build();
}
