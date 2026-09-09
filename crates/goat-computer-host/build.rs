use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=DEVELOPER_DIR");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let output = Command::new("xcrun")
        .args(["swiftc", "-print-target-info"])
        .output()
        .expect("Swift is required for the macOS computer provider");
    assert!(
        output.status.success(),
        "cannot discover Swift runtime libraries"
    );
    let info: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("Swift target info must be JSON");
    for path in info["paths"]["runtimeLibraryPaths"]
        .as_array()
        .expect("Swift runtime paths")
    {
        println!(
            "cargo:rustc-link-search=native={}",
            path.as_str().expect("Swift runtime path")
        );
    }
}
