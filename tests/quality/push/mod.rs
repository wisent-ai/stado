//! Real Git pushes to an isolated bare remote through the real Stado checker.
//! A push runs no gate: format and lint gates that refuse every tree are
//! declared, and only the committed source decides, a module the crate
//! declares and the commit lacks refusing; a push elsewhere: `mirror.rs`.
mod fixture;
mod mirror;
use fixture::Fixture;
use serde_json::{json, Value};
use std::fs;
use std::process::Command;

#[test]
fn a_push_checks_its_commits_not_head_or_working_tree() {
    let mut fixture = Fixture::new();
    fixture.success(
        fixture
            .git()
            .arg("init")
            .arg("--quiet")
            .arg("--initial-branch=main"),
    );
    // Use the account's configured test commit identity, never an embedded account.
    for key in ["user.name", "user.email"] {
        let configured = fixture.success(
            Command::new("git")
                .current_dir(&fixture.repository)
                .arg("config")
                .arg("--get")
                .arg(key),
        );
        let value = String::from_utf8(configured.stdout).expect("Git identity");
        fixture.success(fixture.git().arg("config").arg(key).arg(value.trim()));
    }
    fixture.success(
        Command::new("git")
            .arg("init")
            .arg("--quiet")
            .arg("--bare")
            .arg(&fixture.remote),
    );
    fixture.success(
        fixture
            .git()
            .arg("remote")
            .arg("add")
            .arg("origin")
            .arg(&fixture.remote),
    );
    fixture.success(fixture.stado().arg("config").arg("init"));
    let own: Value = serde_json::from_slice(
        &fs::read(fixture.repository.join(".wisent-release.json"))
            .expect("read Stado release declaration"),
    )
    .expect("parse release declaration");
    // The declared format and lint gates refuse every tree: a push runs
    // neither, so only the committed source decides.
    let mut platforms = serde_json::Map::new();
    for platform in own["platforms"]
        .as_object()
        .expect("declared platforms")
        .keys()
    {
        platforms.insert(
            platform.clone(),
            json!({
                "runner_platform": platform,
                "quality": [
                    {"name": "style", "kind": "format", "argv": ["sh", "-c", "exit 4"]},
                    {"name": "verdict", "kind": "lint", "argv": ["sh", "-c", "exit 3"]}
                ],
                "build": {"argv": ["cat", "VERSION"]}, "stage": {"VERSION": "VERSION"}
            }),
        );
    }
    let manifest = serde_json::to_vec_pretty(&json!({
        "schema_version": own["schema_version"], "product": "quality-push-fixture",
        "releases": true, "version_source": {"kind": "text", "path": "VERSION"},
        "platforms": platforms, "promotion": {"channels": ["candidate"], "reconcile": false}
    }))
    .expect("encode real fixture declaration");
    let manifest_path = fixture.checkout.join(".wisent-release.json");
    let sources = fixture.checkout.join("src");
    fs::create_dir_all(&sources).expect("create the crate's source directory");
    let source_path = sources.join("lib.rs");
    fs::write(&manifest_path, &manifest).expect("declare refusing gates");
    fs::write(fixture.checkout.join("VERSION"), env!("CARGO_PKG_VERSION"))
        .expect("write declared version");
    fs::write(
        fixture.checkout.join("Cargo.toml"),
        "[package]\nname = \"quality-push-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .expect("write the crate manifest");
    fs::write(
        fixture.checkout.join("Cargo.lock"),
        "version = 3\n\n[[package]]\nname = \"quality-push-fixture\"\nversion = \"0.1.0\"\n",
    )
    .expect("write the crate lock");
    // Unformatted, which the refusing format gate would not let through.
    let declared = "pub fn measured()->bool{true}\n";
    fs::write(&source_path, declared).expect("write real Rust source");
    let good = fixture.commit();
    // A module the crate declares and the commit does not hold: compiled,
    // it fails everywhere, and the source check reads it without compiling.
    fs::write(&source_path, format!("mod missing;\n{declared}")).expect("declare a missing module");
    let bad = fixture.commit();
    assert_ne!(good, bad, "the two committed trees must differ");
    // HEAD now declares a module it does not hold, while the uncommitted
    // module file rescues it and the uncommitted manifest disables releases.
    // Neither may affect a push.
    let rescue = sources.join("missing.rs");
    fs::write(&rescue, "").expect("leave another writer's uncommitted module");
    fs::write(&manifest_path, serde_json::to_vec(&json!({
        "schema_version": own["schema_version"], "product": "quality-push-fixture", "releases": false
    })).expect("encode uncommitted declaration")).expect("leave unrelated dirty declaration");
    let dirty_manifest = fs::read(&manifest_path).expect("snapshot dirty declaration");
    let dirty_source = fs::read(&source_path).expect("snapshot the working source");
    let hooks = fixture.root.join("hooks");
    fs::create_dir(&hooks).expect("isolated hooks");
    fs::copy(
        fixture.repository.join("tests/quality/push/pre-push"),
        hooks.join("pre-push"),
    )
    .expect("install tracked test hook in isolated repository");
    fixture.success(
        fixture
            .git()
            .arg("config")
            .arg("core.hooksPath")
            .arg(&hooks),
    );

    let pushed = fixture.success(
        fixture
            .git()
            .arg("push")
            .arg("origin")
            .arg(format!("{good}:refs/heads/main"))
            .arg(format!("{good}:refs/heads/review")),
    );
    let transcript = format!(
        "{}{}",
        String::from_utf8_lossy(&pushed.stdout),
        String::from_utf8_lossy(&pushed.stderr)
    );
    assert!(
        transcript.contains(&good),
        "push did not identify checked commit: {transcript}"
    );
    assert_eq!(
        fixture.published(),
        good,
        "remote must receive the explicitly pushed commit, not HEAD"
    );
    let refused = fixture.run(
        fixture
            .git()
            .arg("push")
            .arg("origin")
            .arg(format!("{bad}:refs/heads/main")),
    );
    assert!(
        !refused.status.success(),
        "working-tree fixes cannot rescue a bad pushed commit"
    );
    let transcript = format!(
        "{}{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert!(
        transcript.contains(&bad) && transcript.contains("missing"),
        "refusal did not identify its committed source and the missing module: {transcript}"
    );
    assert_eq!(
        fixture.published(),
        good,
        "refused push changed remote state"
    );
    mirror::pushed_elsewhere(&mut fixture, &good, &bad);
    // Git sends a null local object for a deleted ref. A bad HEAD must not
    // turn deletion of an already-published branch into a source gate.
    let review = fixture.success(
        Command::new("git")
            .arg("--git-dir")
            .arg(&fixture.remote)
            .arg("rev-parse")
            .arg("refs/heads/review"),
    );
    assert_eq!(
        String::from_utf8(review.stdout)
            .expect("published review ref")
            .trim(),
        good
    );
    fixture.success(
        fixture
            .git()
            .arg("push")
            .arg("origin")
            .arg(":refs/heads/review"),
    );
    let deleted = fixture.success(
        Command::new("git")
            .arg("--git-dir")
            .arg(&fixture.remote)
            .arg("for-each-ref")
            .arg("--format=%(objectname)")
            .arg("refs/heads/review"),
    );
    assert!(
        deleted.stdout.is_empty(),
        "the remote still holds the deleted ref"
    );
    assert_eq!(
        fixture.published(),
        good,
        "deletion changed another remote ref"
    );

    // A notes commit carries notes about commits, not a product tree: pushing
    // it with a bad HEAD succeeds, says so, and publishes the notes.
    fixture.success(
        fixture
            .git()
            .arg("notes")
            .arg("--ref=attestation")
            .arg("add")
            .arg("--message=checked where its branch is pushed")
            .arg(&good),
    );
    let noted = fixture.success(
        fixture
            .git()
            .arg("push")
            .arg("origin")
            .arg("refs/notes/attestation"),
    );
    let transcript = format!(
        "{}{}",
        String::from_utf8_lossy(&noted.stdout),
        String::from_utf8_lossy(&noted.stderr)
    );
    assert!(
        transcript.contains("refs/notes/attestation holds notes, not a source tree"),
        "the notes push was not said to hold notes: {transcript}"
    );
    let local_notes = fixture.success(fixture.git().arg("rev-parse").arg("refs/notes/attestation"));
    let remote_notes = fixture.success(
        Command::new("git")
            .arg("--git-dir")
            .arg(&fixture.remote)
            .arg("rev-parse")
            .arg("refs/notes/attestation"),
    );
    assert_eq!(
        remote_notes.stdout, local_notes.stdout,
        "the remote does not hold the pushed notes"
    );

    // A real blob exists but is not a commit: explicit revision selection
    // refuses it instead of quietly choosing HEAD or another source tree.
    let blob = fixture.success(
        fixture
            .git()
            .arg("rev-parse")
            .arg(format!("{good}:src/lib.rs")),
    );
    let blob = String::from_utf8(blob.stdout).expect("source blob identity");
    let invalid = fixture.run(
        fixture
            .stado()
            .arg("quality")
            .arg("check")
            .arg("--revision")
            .arg(blob.trim()),
    );
    assert!(
        !invalid.status.success(),
        "a source blob is not a checkable commit"
    );
    let refusal = format!(
        "{}{}",
        String::from_utf8_lossy(&invalid.stdout),
        String::from_utf8_lossy(&invalid.stderr)
    );
    assert!(
        refusal.contains(blob.trim()),
        "refusal loses the selected object: {refusal}"
    );
    assert_eq!(
        fs::read(&source_path).expect("read working source"),
        dirty_source
    );
    assert!(rescue.is_file(), "the push removed the uncommitted module");
    assert_eq!(
        fs::read(&manifest_path).expect("read working declaration"),
        dirty_manifest
    );
    fixture.passed();
}
