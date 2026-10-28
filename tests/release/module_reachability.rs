//! Run with `cargo test --manifest-path stado-rs/Cargo.toml --test release-module-reachability -- --nocapture`.
//!
//! `stado release gate modules unreachable --crate <dir> --json` over a real
//! crate of this test's own under the repository's `.build`: a file reached
//! through `#[path]` and one reached through an inline module count as
//! reached; a file whose only "declaration" sits inside a string, and a file
//! nothing declares, are reported unreachable and the command fails; a
//! reachable file that is not Rust is refused naming that file, never read as
//! declaring nothing. The binary is the one Cargo built for this test; its
//! home is this run's own.
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn gate(root: &Path, crate_dir: &Path, report: &mut Value) -> Output {
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_stado"));
    let args = ["release", "gate", "modules", "unreachable", "--crate"];
    let output = Command::new(&binary)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", root.join("home"))
        .env("STADO_CONFIG", root.join("home/.stado/config.json"))
        .stdin(Stdio::null())
        .args(args)
        .arg(crate_dir)
        .arg("--json")
        .output()
        .unwrap();
    report["commands"].as_array_mut().unwrap().push(json!({
        "args": args,
        "crate": crate_dir.display().to_string(),
        "exit_status": output.status.code(),
        "stdout": String::from_utf8_lossy(&output.stdout),
        "stderr": String::from_utf8_lossy(&output.stderr),
    }));
    output
}

#[test]
fn reachability_follows_rust_declarations_and_refuses_what_is_not_rust() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let build = repository.join(".build");
    fs::create_dir_all(&build).unwrap();
    let root = build.join(format!("module-reachability-{}", std::process::id()));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    fs::create_dir_all(root.join("home/.stado")).unwrap();
    let mut report = json!({"commands": [], "cases": []});

    let fixture = root.join("fixture");
    write(
        &fixture,
        "Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(
        &fixture,
        "src/lib.rs",
        "mod reached;\n#[path = \"elsewhere/renamed.rs\"]\nmod renamed;\nmod inline {\n    mod deep;\n}\npub const TEXT: &str = \"mod quoted;\";\n",
    );
    write(&fixture, "src/reached.rs", "pub fn reached() {}\n");
    write(
        &fixture,
        "src/elsewhere/renamed.rs",
        "pub fn renamed() {}\n",
    );
    write(&fixture, "src/inline/deep.rs", "pub fn deep() {}\n");
    write(&fixture, "src/quoted.rs", "pub fn quoted() {}\n");
    write(&fixture, "src/orphan.rs", "pub fn orphan() {}\n");

    let findings = gate(&root, &fixture, &mut report);
    assert!(
        !findings.status.success(),
        "unreachable files must fail the gate"
    );
    let answer: Value = serde_json::from_slice(&findings.stdout).unwrap();
    let unreachable: Vec<&str> = answer["unreachable"]
        .as_array()
        .unwrap()
        .iter()
        .map(|path| path.as_str().unwrap())
        .collect();
    assert_eq!(unreachable, ["src/orphan.rs", "src/quoted.rs"], "{answer}");
    report["cases"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "declarations-as-rustc-reads-them", "result": "passed"}));

    write(&fixture, "src/reached.rs", "pub fn reached( {\n");
    let refused = gate(&root, &fixture, &mut report);
    assert!(
        !refused.status.success(),
        "a reachable file that is not Rust must be refused"
    );
    let said = String::from_utf8_lossy(&refused.stderr);
    assert!(
        said.contains("reached.rs") && said.contains("cannot parse"),
        "{said}"
    );
    report["cases"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name": "unparsable-reachable-file", "result": "passed"}));

    fs::write(
        root.join("report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    println!("passed: {}", root.join("report.json").display());
}
