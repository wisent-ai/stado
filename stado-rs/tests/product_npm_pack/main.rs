//! `stado product npm pack` through the real binary and the real `npm` on a
//! scratch package: the one packed artifact is staged as
//! `release/npm-package.tgz` with its digest beside it, the package's own
//! lifecycle scripts do not run, and a build without a source is refused.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(label: &str) -> (PathBuf, PathBuf) {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("npm-pack-{label}"));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    let source = root.join("source");
    fs::create_dir_all(&source).unwrap();
    fs::write(
        source.join("package.json"),
        r#"{"name":"stado-npm-pack-fixture","version":"1.2.3","main":"index.js",
            "scripts":{"prepack":"node -e \"require('fs').writeFileSync('ran-prepack','')\""}}"#,
    )
    .unwrap();
    fs::write(source.join("index.js"), b"module.exports = 1\n").unwrap();
    (source, root.join("output"))
}

fn pack(source: Option<&Path>, output: &Path) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    command
        .args(["product", "npm", "pack"])
        .env("WISENT_OUTPUT_DIR", output)
        .env_remove("WISENT_SOURCE_DIR");
    if let Some(source) = source {
        command.env("WISENT_SOURCE_DIR", source);
    }
    command.output().expect("run stado product npm pack")
}

#[test]
fn a_package_is_packed_once_with_its_digest_and_no_scripts() {
    let (source, output) = scratch("packed");
    let run = pack(Some(&source), &output);
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let package = output.join("release/npm-package.tgz");
    assert!(package.is_file(), "no staged package");
    let digest = fs::read_to_string(output.join("release/npm-package.tgz.sha256")).unwrap();
    let listed = Command::new("shasum")
        .args(["-a", "256"])
        .arg(&package)
        .output()
        .unwrap();
    let expected = String::from_utf8_lossy(&listed.stdout);
    assert_eq!(digest.trim(), expected.split_whitespace().next().unwrap());
    let members = Command::new("tar").arg("-tzf").arg(&package).output().unwrap();
    let members = String::from_utf8_lossy(&members.stdout);
    assert!(members.contains("package/index.js"), "{members}");
    assert!(
        !source.join("ran-prepack").exists(),
        "npm pack ran the package's prepack script"
    );
}

#[test]
fn a_build_without_a_source_is_refused_and_nothing_is_staged() {
    let (_, output) = scratch("refused");
    let run = pack(None, &output);
    assert!(!run.status.success());
    assert!(
        String::from_utf8_lossy(&run.stderr).contains("WISENT_SOURCE_DIR"),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(!output.join("release/npm-package.tgz").exists());
}
