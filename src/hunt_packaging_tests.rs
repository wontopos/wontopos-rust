#[test]
fn the_published_crate_leaves_out_every_test_only_file() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = std::process::Command::new(cargo)
        .args(["package", "--list", "--allow-dirty", "--offline"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let listed = String::from_utf8_lossy(&out.stdout);
    let test_only: Vec<&str> = listed
        .lines()
        .filter(|f| {
            f.ends_with("_tests.rs")
                || *f == "src/tests.rs"
                || f.starts_with("src/testdata/")
                || f.starts_with("src/sleep/tests/")
        })
        .collect();
    assert!(test_only.is_empty(), "test-only files in the crate: {test_only:?}");
    for kept in ["src/lib.rs", "src/sleep.rs", "src/router.rs", "src/memory.rs", "src/types.rs", "README.md"] {
        assert!(listed.lines().any(|f| f == kept), "{kept} is missing from the crate");
    }
}

#[test]
fn the_manifest_names_the_oldest_rust_the_crate_builds_with() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = std::process::Command::new(cargo)
        .args(["metadata", "--no-deps", "--offline", "--format-version", "1"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let declared = meta["packages"][0]["rust_version"].as_str().unwrap_or("");
    let parts: Vec<u32> = declared.split('.').filter_map(|p| p.parse().ok()).collect();
    assert!(
        parts.len() >= 2 && (parts[0], parts[1]) >= (1, 83),
        "rust-version {declared:?}: the crate and its dependencies need Rust 1.83 or newer"
    );
}
