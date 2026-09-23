//! `stado release catalog adopt --kind ios-xcode` against real git checkouts
//! holding an Xcode project, with the local storage backend standing in for
//! the release store. What is checked is what an operator sees: the preview
//! writes nothing, --apply writes a manifest the pipeline parses and scripts
//! filled from the project and registers the product, and a checkout the
//! command cannot read is refused with the reason.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const PBXPROJ: &str = "// !$*UTF8*$!\n{\n\tobjects = {\n\
\t\tA = {\n\t\t\tbuildSettings = {\n\t\t\t\tDEVELOPMENT_TEAM = TEAM123456;\n\
\t\t\t\tMARKETING_VERSION = 1.0;\n\t\t\t\tPRODUCT_BUNDLE_IDENTIFIER = com.example.demo;\n\t\t\t};\n\t\t};\n\
\t\tB = {\n\t\t\tbuildSettings = {\n\t\t\t\tMARKETING_VERSION = 1.0;\n\
\t\t\t\tPRODUCT_BUNDLE_IDENTIFIER = com.example.DemoTests;\n\t\t\t};\n\t\t};\n\t};\n}\n";

fn stado(storage: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(args)
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("STADO_CONFIG", storage.join("no-such-config.json"))
        .env_remove("COMPUTE_API_KEY")
        .env_remove("COMPUTE_API_URL")
        .output()
        .expect("stado binary runs")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A git checkout named `demo-ios` holding Demo.xcodeproj with `pbxproj`.
fn checkout(root: &Path, pbxproj: &str) -> PathBuf {
    let repo = root.join("demo-ios");
    std::fs::create_dir_all(repo.join("Demo.xcodeproj")).unwrap();
    std::fs::write(repo.join("Demo.xcodeproj/project.pbxproj"), pbxproj).unwrap();
    let init = Command::new("git").args(["init", "-q"]).arg(&repo).status().unwrap();
    assert!(init.success(), "git init failed");
    repo
}

#[test]
fn the_preview_names_every_file_and_writes_none() {
    let dir = tempfile::tempdir().unwrap();
    let repo = checkout(dir.path(), PBXPROJ);
    let out = stado(dir.path(), &["release", "catalog", "adopt", repo.to_str().unwrap(), "--kind", "ios-xcode"]);
    assert!(out.status.success(), "preview failed: {}", text(&out.stderr));
    let printed = text(&out.stdout);
    for name in [".wisent-release.json", "release/build.sh", "release/quality.sh", "release/archive-tree.py"] {
        assert!(printed.contains(&format!("would write {name}")), "{name} missing from: {printed}");
    }
    assert!(text(&out.stderr).contains("bundle com.example.demo, team TEAM123456, version 1.0"));
    assert!(!repo.join(".wisent-release.json").exists(), "the preview wrote the manifest");
    assert!(!repo.join("release").exists(), "the preview wrote scripts");
}

#[test]
fn apply_writes_scripts_filled_from_the_project_and_registers_it() {
    let dir = tempfile::tempdir().unwrap();
    let repo = checkout(dir.path(), PBXPROJ);
    let out = stado(
        dir.path(),
        &["release", "catalog", "adopt", repo.to_str().unwrap(), "--kind", "ios-xcode", "--apply"],
    );
    assert!(out.status.success(), "apply failed: {}", text(&out.stderr));
    assert!(text(&out.stdout).contains("cataloged demo-ios"), "not registered: {}", text(&out.stdout));
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(repo.join(".wisent-release.json")).unwrap()).unwrap();
    assert_eq!(manifest["product"], "demo-ios");
    assert_eq!(manifest["version_source"]["path"], "Demo.xcodeproj/project.pbxproj");
    assert_eq!(
        manifest["platforms"]["ios-arm64"]["secret_env"]["IOS_PROFILE_B64"],
        "demo-ios-signing#provisioning_profile_base64"
    );
    let build = std::fs::read_to_string(repo.join("release/build.sh")).unwrap();
    assert!(build.contains("<key>com.example.demo</key>"), "the export names another bundle");
    assert!(build.contains("-scheme \"Demo\"") && build.contains("APPLE_TEAM_ID:-TEAM123456"));
    assert!(!build.contains("{{"), "a placeholder was left unfilled");
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(repo.join("release/build.sh")).unwrap().permissions().mode();
    assert_eq!(mode & 0o111, 0o111, "build.sh is not executable");
}

#[test]
fn a_checkout_that_declares_a_manifest_is_refused_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let repo = checkout(dir.path(), PBXPROJ);
    std::fs::write(repo.join(".wisent-release.json"), "{}").unwrap();
    let out = stado(dir.path(), &["release", "catalog", "adopt", repo.to_str().unwrap(), "--kind", "ios-xcode", "--apply"]);
    assert!(!out.status.success());
    assert!(text(&out.stderr).contains("already declares .wisent-release.json"), "{}", text(&out.stderr));
    assert!(!repo.join("release").exists(), "a refused checkout was written to");
}

#[test]
fn a_checkout_without_a_project_or_a_readable_version_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let bare = dir.path().join("plain");
    std::fs::create_dir_all(bare.join(".git")).unwrap();
    let out = stado(dir.path(), &["release", "catalog", "adopt", bare.to_str().unwrap(), "--kind", "ios-xcode"]);
    assert!(!out.status.success());
    assert!(text(&out.stderr).contains("has no .xcodeproj at its root"), "{}", text(&out.stderr));

    let repo = checkout(&dir.path().join("other"), &PBXPROJ.replace("MARKETING_VERSION = 1.0;", "MARKETING_VERSION = \"$(VERSION)\";"));
    let out = stado(dir.path(), &["release", "catalog", "adopt", repo.to_str().unwrap(), "--kind", "ios-xcode"]);
    assert!(!out.status.success());
    assert!(text(&out.stderr).contains("MARKETING_VERSION as two or three numbers"), "{}", text(&out.stderr));
}
