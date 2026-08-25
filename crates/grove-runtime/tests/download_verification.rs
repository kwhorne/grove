//! Checksum verification against what publishers actually serve today.
//!
//! These reach the network, so they are `#[ignore]`d and not part of the normal
//! suite. They exist because the verification logic is only as good as its
//! agreement with the real documents — a parser that handles my idea of
//! `SHASUMS256.txt` and not Node's would pass every unit test and fail every
//! install.
//!
//! ```console
//! $ cargo test -p grove-runtime --test download_verification -- --ignored --nocapture
//! ```

use grove_core::paths::GrovePaths;

fn scratch(name: &str) -> GrovePaths {
    let base = std::env::temp_dir().join(format!("grove-dlverify-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    GrovePaths::with_base(base)
}

/// The full chain for Node: download the tarball, fetch `SHASUMS256.txt`, find
/// the line for this platform's file, verify, then unpack.
///
/// If the hash were computed wrongly, the manifest parsed wrongly, or the wrong
/// line matched, this fails rather than installing something unverified.
#[test]
#[ignore = "downloads ~30 MB from nodejs.org"]
fn node_is_installed_only_after_its_checksum_matches() {
    let paths = scratch("node");
    let mut registry = grove_runtime::NodeRegistry::load(&paths);
    let build = grove_runtime::install_node(&paths, &mut registry, "22", |m| {
        eprintln!("  {m}");
    })
    .expect("Node should install and verify");

    assert!(
        build.node_binary.exists(),
        "the node binary should be there"
    );
    let out = std::process::Command::new(&build.node_binary)
        .arg("--version")
        .output()
        .expect("node should run");
    let version = String::from_utf8_lossy(&out.stdout);
    assert!(version.trim().starts_with("v22."), "got {version:?}");

    let _ = std::fs::remove_dir_all(paths.base());
}

/// Composer's PHAR against `composer-stable.phar.sha256`.
#[test]
#[ignore = "downloads composer.phar from getcomposer.org"]
fn composer_is_verified_before_it_is_written() {
    let paths = scratch("composer");
    let phar = grove_runtime::scaffold::ensure_composer(&paths)
        .expect("composer should download and verify");
    assert!(phar.exists());
    // A PHAR starts with a PHP stub; a verified download of an error page would
    // be a verification that checked the wrong thing.
    let head = std::fs::read(&phar).unwrap();
    let head = String::from_utf8_lossy(&head[..head.len().min(32)]).to_string();
    assert!(head.contains("php"), "not a PHAR: {head:?}");
    let _ = std::fs::remove_dir_all(paths.base());
}

/// The published documents still have the shape the parser expects.
///
/// Cheap — no artefacts, just the manifests — and it is the check that would
/// catch a publisher changing format from under us.
#[test]
#[ignore = "reaches nodejs.org and getcomposer.org"]
fn published_checksum_documents_still_parse() {
    let node_version = "24.10.0";
    let sums = ureq::get(&format!(
        "https://nodejs.org/dist/v{node_version}/SHASUMS256.txt"
    ))
    .call()
    .expect("SHASUMS256.txt")
    .into_string()
    .unwrap();
    let name = format!("node-v{node_version}-darwin-arm64.tar.gz");
    let hash = grove_core::checksum::expected_for(&sums, &name)
        .unwrap_or_else(|| panic!("no line for {name} in SHASUMS256.txt"));
    assert_eq!(hash.len(), 64, "not a sha256: {hash}");

    let composer = ureq::get("https://getcomposer.org/composer-stable.phar.sha256")
        .call()
        .expect("composer sha256")
        .into_string()
        .unwrap();
    let hash = grove_core::checksum::expected_for(&composer, "composer-stable.phar")
        .expect("composer publishes a bare digest");
    assert_eq!(hash.len(), 64, "not a sha256: {hash}");
}
