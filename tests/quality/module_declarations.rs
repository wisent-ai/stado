//! Run with `cargo test --manifest-path stado-rs/Cargo.toml --test quality-path-dependency module_declarations -- --nocapture`.
//!
//! `stado quality check` over a Cargo product whose committed tree declares a
//! module file it does not hold refuses the commit naming the declaring file
//! and the files it looked for, before any gate runs and with nothing
//! compiled; the same product once the file is committed is not refused by
//! that check. A tree whose code names `crate::hidden` while `hidden/mod.rs`
//! is committed but declared by nothing is refused the same way, naming the
//! using file, the path and the undeclared file, until `mod hidden;` is
//! committed. A path naming the item `crate::hidden::Item` beside the module
//! file `hidden/item.rs` is not refused: on a case-insensitive filesystem
//! `Item.rs` opens `item.rs`, which is a declared module, not the item. The
//! checkout is a real Git repository of this test's own under the
//! repository's `.build`, with a tracked change left in it so the check reads
//! `HEAD` rather than an origin; Stado's home and store are this run's own.
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const MANIFEST: &str = ".wisent-release.json";
const REFUSAL: &str = "declares module files it does not hold";
const UNREACHED: &str = "uses modules no declaration reaches";

fn git(directory: &Path, args: &[&str]) {
    let status = Command::new("git")
        // The operator's global Git configuration, its hooks path included,
        // never judges the fixture's own history; the fixture's commits
        // carry their own author.
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Stado quality fixture")
        .env("GIT_AUTHOR_EMAIL", "")
        .env("GIT_COMMITTER_NAME", "Stado quality fixture")
        .env("GIT_COMMITTER_EMAIL", "")
        .args(args)
        .current_dir(directory)
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} in {}", directory.display());
}

fn stado(root: &Path, report: &mut Value, args: &[&str]) -> Output {
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_stado"));
    let output = Command::new(&binary)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", root.join("home"))
        .env("STADO_CONFIG", root.join("home/.stado/config.json"))
        .env("TMPDIR", root.join("tmp"))
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", root.join("store"))
        .stdin(Stdio::null())
        .args(args)
        .output()
        .unwrap();
    report["commands"].as_array_mut().unwrap().push(json!({
        "args": args,
        "exit_status": output.status.code(),
        "stdout": String::from_utf8_lossy(&output.stdout),
        "stderr": String::from_utf8_lossy(&output.stderr),
    }));
    fs::write(
        root.join("report.json"),
        serde_json::to_vec_pretty(report).unwrap(),
    )
    .unwrap();
    output
}

#[test]
fn a_module_declared_without_its_file_is_refused_before_anything_compiles() {
    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf();
    let root = repository
        .join(".build/quality-module-declarations")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(root.join("tmp")).unwrap();
    fs::create_dir_all(root.join("home")).unwrap();
    fs::create_dir_all(root.join("store")).unwrap();
    let mut report = json!({ "commands": [], "outcome": "failed" });
    let checkout = root.join("product");
    fs::create_dir_all(checkout.join("src")).unwrap();
    git(&checkout, &["init", "--quiet", "--initial-branch=main"]);
    let own: Value = serde_json::from_slice(&fs::read(repository.join(MANIFEST)).unwrap()).unwrap();
    let manifest = json!({
        "schema_version": own["schema_version"],
        "product": "quality-module-declarations-test",
        "releases": true,
        "version_source": {
            "kind": "regex", "path": "Cargo.toml",
            "pattern": "(?m)^version\\s*=\\s*\"(?P<version>[^\"]+)\"\\s*$"
        },
        "platforms": own["platforms"],
        "promotion": {"channels": ["candidate"], "reconcile": false}
    });
    fs::write(
        checkout.join(MANIFEST),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();
    fs::write(
        checkout.join("Cargo.toml"),
        "[package]\nname = \"product\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        checkout.join("src/main.rs"),
        "mod present;\nmod absent;\nfn main() {}\n",
    )
    .unwrap();
    fs::write(checkout.join("src/present.rs"), "").unwrap();
    fs::write(checkout.join("NOTES"), "first\n").unwrap();
    git(&checkout, &["add", "."]);
    git(
        &checkout,
        &[
            "commit",
            "--quiet",
            "-m",
            "declares absent without its file",
        ],
    );
    fs::write(
        checkout.join("NOTES"),
        "tracked change, so the check reads HEAD\n",
    )
    .unwrap();

    let refused = stado(
        &root,
        &mut report,
        &["quality", "check", "--root", &checkout.to_string_lossy()],
    );
    let said = String::from_utf8_lossy(&refused.stderr).into_owned();
    assert!(
        !refused.status.success(),
        "a tree declaring a missing module was accepted"
    );
    assert!(said.contains(REFUSAL), "{said}");
    assert!(
        said.contains("src/main.rs declares `mod absent;`"),
        "{said}"
    );
    assert!(said.contains("src/absent.rs or"), "{said}");
    assert!(!said.contains("mod present;"), "{said}");

    fs::write(checkout.join("src/absent.rs"), "").unwrap();
    git(&checkout, &["add", "src/absent.rs"]);
    git(
        &checkout,
        &["commit", "--quiet", "-m", "adds the declared file"],
    );
    let checked = stado(
        &root,
        &mut report,
        &["quality", "check", "--root", &checkout.to_string_lossy()],
    );
    let said = String::from_utf8_lossy(&checked.stderr).into_owned();
    assert!(
        !said.contains(REFUSAL),
        "the module check refused a complete tree: {said}"
    );

    fs::create_dir_all(checkout.join("src/hidden")).unwrap();
    fs::write(checkout.join("src/hidden/mod.rs"), "pub fn run() {}\n").unwrap();
    fs::write(
        checkout.join("src/main.rs"),
        "mod present;\nmod absent;\nfn main() {\n    crate::hidden::run();\n}\n",
    )
    .unwrap();
    git(&checkout, &["add", "src"]);
    git(
        &checkout,
        &[
            "commit",
            "--quiet",
            "-m",
            "uses hidden without declaring it",
        ],
    );
    let hidden = stado(
        &root,
        &mut report,
        &["quality", "check", "--root", &checkout.to_string_lossy()],
    );
    let said = String::from_utf8_lossy(&hidden.stderr).into_owned();
    assert!(
        !hidden.status.success(),
        "a path into an undeclared module was accepted"
    );
    assert!(said.contains(UNREACHED), "{said}");
    assert!(
        said.contains("src/main.rs uses `crate::hidden`, and the tree holds src/hidden/mod.rs"),
        "{said}"
    );

    fs::write(
        checkout.join("src/hidden/mod.rs"),
        "pub mod item;\npub use item::Item;\npub fn run() {}\n",
    )
    .unwrap();
    fs::write(checkout.join("src/hidden/item.rs"), "pub struct Item;\n").unwrap();
    fs::write(
        checkout.join("src/main.rs"),
        "mod present;\nmod absent;\nmod hidden;\nfn main() {\n    crate::hidden::run();\n    let _item = crate::hidden::Item;\n}\n",
    )
    .unwrap();
    git(&checkout, &["add", "src"]);
    git(&checkout, &["commit", "--quiet", "-m", "declares hidden"]);
    let declared = stado(
        &root,
        &mut report,
        &["quality", "check", "--root", &checkout.to_string_lossy()],
    );
    let said = String::from_utf8_lossy(&declared.stderr).into_owned();
    assert!(
        !said.contains(UNREACHED) && !said.contains(REFUSAL),
        "the module check refused a declared module: {said}"
    );

    // A declaration inside an inline module resolves under the inline
    // module's name, as rustc resolves it: refused while that file is
    // missing, accepted once it is committed.
    fs::write(
        checkout.join("src/main.rs"),
        "mod present;\nmod absent;\nmod hidden;\nmod outer {\n    pub mod inner;\n}\nfn main() {\n    crate::hidden::run();\n    crate::outer::inner::run();\n}\n",
    )
    .unwrap();
    git(&checkout, &["add", "src"]);
    git(
        &checkout,
        &["commit", "--quiet", "-m", "declares inner inside outer"],
    );
    let nested = stado(
        &root,
        &mut report,
        &["quality", "check", "--root", &checkout.to_string_lossy()],
    );
    let said = String::from_utf8_lossy(&nested.stderr).into_owned();
    assert!(
        !nested.status.success(),
        "a missing file inside an inline module was accepted"
    );
    assert!(said.contains("src/main.rs declares `mod inner;`"), "{said}");
    assert!(said.contains("src/outer/inner.rs or"), "{said}");
    fs::create_dir_all(checkout.join("src/outer")).unwrap();
    fs::write(checkout.join("src/outer/inner.rs"), "pub fn run() {}\n").unwrap();
    git(&checkout, &["add", "src"]);
    git(
        &checkout,
        &["commit", "--quiet", "-m", "adds inner under outer"],
    );
    let inline = stado(
        &root,
        &mut report,
        &["quality", "check", "--root", &checkout.to_string_lossy()],
    );
    let said = String::from_utf8_lossy(&inline.stderr).into_owned();
    assert!(
        !said.contains(UNREACHED) && !said.contains(REFUSAL),
        "the module check refused a module declared inside an inline module: {said}"
    );
    report["outcome"] = json!("passed");
    fs::write(
        root.join("report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
}
