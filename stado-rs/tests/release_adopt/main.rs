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

/// `stado release catalog adopt <checkout> --kind ios-xcode <extra>` with the
/// local storage backend under `storage`.
fn adopt(storage: &Path, checkout: &Path, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["release", "catalog", "adopt"])
        .arg(checkout)
        .args(["--kind", "ios-xcode"])
        .args(extra)
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
    let init = Command::new("git")
        .args(["init", "-q"])
        .arg(&repo)
        .status()
        .unwrap();
    assert!(init.success(), "git init failed");
    let remote = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["remote", "add", "origin", "https://github.com/wisent-ai/demo-ios.git"])
        .status()
        .unwrap();
    assert!(remote.success(), "git remote add failed");
    repo
}

#[test]
fn the_preview_names_every_file_and_writes_none() {
    let dir = tempfile::tempdir().unwrap();
    let repo = checkout(dir.path(), PBXPROJ);
    let out = adopt(dir.path(), &repo, &[]);
    assert!(out.status.success(), "preview: {}", text(&out.stderr));
    let printed = text(&out.stdout);
    for name in [
        ".wisent-release.json",
        "release/build.sh",
        "release/quality.sh",
        "release/archive-tree.py",
    ] {
        assert!(
            printed.contains(&format!("would write {name}")),
            "{name} missing from: {printed}"
        );
    }
    let said = text(&out.stderr);
    assert!(said.contains("bundle com.example.demo, team TEAM123456, version 1.0"));
    assert!(!repo.join(".wisent-release.json").exists());
    assert!(!repo.join("release").exists(), "the preview wrote scripts");
}

#[test]
fn a_misnamed_checkout_refuses_to_register_the_wrong_origin() {
    let dir = tempfile::tempdir().unwrap();
    let repo = checkout(dir.path(), PBXPROJ);
    let remote = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["remote", "set-url", "origin", "https://github.com/wisent-ai/other-ios.git"])
        .status()
        .unwrap();
    assert!(remote.success());

    let out = adopt(dir.path(), &repo, &["--apply"]);
    assert!(!out.status.success());
    let said = text(&out.stderr);
    assert!(said.contains("origin https://github.com/wisent-ai/other-ios.git names other-ios"), "{said}");
    assert!(!repo.join(".wisent-release.json").exists());
    assert!(!repo.join("release").exists());
}

#[test]
fn a_checkout_without_an_origin_refuses_apply_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let repo = checkout(dir.path(), PBXPROJ);
    let removed = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["remote", "remove", "origin"])
        .status()
        .unwrap();
    assert!(removed.success());

    let out = adopt(dir.path(), &repo, &["--apply"]);
    assert!(!out.status.success());
    assert!(text(&out.stderr).contains("has no readable origin"), "{}", text(&out.stderr));
    assert!(!repo.join(".wisent-release.json").exists());
    assert!(!repo.join("release").exists());
}

#[test]
fn apply_writes_scripts_filled_from_the_project_and_registers_it() {
    let dir = tempfile::tempdir().unwrap();
    let repo = checkout(dir.path(), PBXPROJ);
    let out = adopt(dir.path(), &repo, &["--apply"]);
    assert!(out.status.success(), "apply: {}", text(&out.stderr));
    let printed = text(&out.stdout);
    assert!(printed.contains("cataloged demo-ios"), "{printed}");
    let bytes = std::fs::read(repo.join(".wisent-release.json")).unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(manifest["product"], "demo-ios");
    let source = &manifest["version_source"]["path"];
    assert_eq!(source, "Demo.xcodeproj/project.pbxproj");
    assert_eq!(
        manifest["platforms"]["ios-arm64"]["secret_env"]["IOS_PROFILE_B64"],
        "demo-ios-signing#provisioning_profile_base64"
    );
    let build = std::fs::read_to_string(repo.join("release/build.sh")).unwrap();
    assert!(build.contains("<key>com.example.demo</key>"), "{build}");
    assert!(build.contains("-scheme \"Demo\""), "{build}");
    assert!(build.contains("APPLE_TEAM_ID:-TEAM123456"), "{build}");
    assert!(!build.contains("{{"), "a placeholder was left unfilled");
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::metadata(repo.join("release/build.sh")).unwrap();
    let mode = metadata.permissions().mode();
    assert_eq!(mode & 0o111, 0o111, "build.sh is not executable");
}

#[test]
fn a_checkout_that_declares_a_manifest_is_refused_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let repo = checkout(dir.path(), PBXPROJ);
    std::fs::write(repo.join(".wisent-release.json"), "{}").unwrap();
    let out = adopt(dir.path(), &repo, &["--apply"]);
    assert!(!out.status.success());
    let said = text(&out.stderr);
    assert!(
        said.contains("already declares .wisent-release.json"),
        "{said}"
    );
    assert!(!repo.join("release").exists(), "a refused checkout changed");
}

#[test]
fn a_checkout_without_a_project_or_a_readable_version_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let bare = dir.path().join("plain");
    std::fs::create_dir_all(bare.join(".git")).unwrap();
    let out = adopt(dir.path(), &bare, &[]);
    assert!(!out.status.success());
    let said = text(&out.stderr);
    assert!(said.contains("has no .xcodeproj at its root"), "{said}");

    let unread = PBXPROJ.replace("VERSION = 1.0;", "VERSION = \"$(VERSION)\";");
    let repo = checkout(&dir.path().join("other"), &unread);
    let out = adopt(dir.path(), &repo, &[]);
    assert!(!out.status.success());
    let said = text(&out.stderr);
    assert!(said.contains("MARKETING_VERSION as two or three"), "{said}");
}
