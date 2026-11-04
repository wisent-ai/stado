//! A program Stado signs carries the `com.wisent.<product>` name every
//! Wisent name follows, and one signed earlier under the retired
//! `ai.wisent.<product>.<product>` spelling moves to it instead of being
//! refused as an identity change. Any other change is still refused.
//!
//! The real `stado` this build made signs copies of itself, kept under the
//! checkout's build directory, with the signing identity the operator names
//! for product signing (`WISENT_CODESIGN_KEYCHAIN` and
//! `WISENT_CODESIGN_IDENTITY`); `codesign -dv` reads what each copy carries.
#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn run(command: &mut Command) -> Output {
    command.output().expect("the program runs")
}

fn said(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The identifier the code at `path` carries, as `codesign` reads it.
fn carried(path: &Path) -> String {
    let read = run(Command::new("/usr/bin/codesign").arg("-dv").arg(path));
    said(&read)
        .lines()
        .find_map(|line| line.strip_prefix("Identifier="))
        .unwrap_or_else(|| panic!("codesign names no identifier of {}: {}", path.display(), said(&read)))
        .to_string()
}

/// A copy of the real `stado` at `path`, signed by Stado with `identifier`.
fn signed_copy(path: &Path, identifier: &str) {
    std::fs::copy(env!("CARGO_BIN_EXE_stado"), path).expect("copy the real stado");
    let signed = sign(path, &["--identifier", identifier], None);
    assert!(signed.status.success(), "stado could not sign {} as {identifier}: {}", path.display(), said(&signed));
}

fn sign(path: &Path, naming: &[&str], previous: Option<&Path>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    command.args(["product", "signing", "sign", "--json"]).args(naming);
    if let Some(previous) = previous {
        command.arg("--previous").arg(previous);
    }
    run(command.arg(path))
}

#[test]
fn a_signed_program_carries_the_com_wisent_name() {
    for variable in ["WISENT_CODESIGN_KEYCHAIN", "WISENT_CODESIGN_IDENTITY"] {
        assert!(
            std::env::var_os(variable).is_some(),
            "{variable} names the signing identity this journey signs with; it is not set"
        );
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../.build/signing-convention")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&root).expect("create the journey's directory");

    // A program named after its product, never signed: com.wisent.<product>.
    let fresh = root.join("probe");
    std::fs::copy(env!("CARGO_BIN_EXE_stado"), &fresh).expect("copy the real stado");
    let signed = sign(&fresh, &["--product", "probe"], None);
    assert!(signed.status.success(), "a fresh program was not signed: {}", said(&signed));
    assert_eq!(carried(&fresh), "com.wisent.probe");

    // Another program of the product: com.wisent.<product>.<name>.
    let helper = root.join("probe-helper");
    std::fs::copy(env!("CARGO_BIN_EXE_stado"), &helper).expect("copy the real stado");
    let signed = sign(&helper, &["--product", "probe"], None);
    assert!(signed.status.success(), "a helper program was not signed: {}", said(&signed));
    assert_eq!(carried(&helper), "com.wisent.probe.probe-helper");

    // An installed program under the retired spelling moves to the convention.
    let installed = root.join("installed");
    signed_copy(&installed, "ai.wisent.probe.probe");
    let candidate = root.join("candidate");
    std::fs::copy(env!("CARGO_BIN_EXE_stado"), &candidate).expect("copy the real stado");
    let moved = sign(&candidate, &["--product", "probe"], Some(&installed));
    assert!(moved.status.success(), "the retired spelling was refused: {}", said(&moved));
    assert_eq!(carried(&candidate), "com.wisent.probe");

    // Any other change of identifier is still refused, naming both.
    let other = root.join("other");
    std::fs::copy(env!("CARGO_BIN_EXE_stado"), &other).expect("copy the real stado");
    let refused = sign(&other, &["--identifier", "com.wisent.another"], Some(&installed));
    assert!(!refused.status.success(), "an identity change was signed: {}", said(&refused));
    assert!(
        said(&refused).contains("code identifier would change: \"ai.wisent.probe.probe\" -> com.wisent.another"),
        "the refusal does not name both identifiers: {}",
        said(&refused)
    );

    std::fs::remove_dir_all(&root).expect("remove the journey's directory");
}
