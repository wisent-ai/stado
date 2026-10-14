//! Real Git pushes to an isolated bare remote through the real Stado checker.
//! The gate is rustfmt, not a canned executable or a simulated quality verdict.
mod fixture;
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
                "quality": [{"name": "rust-source", "kind": "format",
                    "argv": ["rustfmt", "--edition", "2021", "--check", "source.rs"]}],
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
    let source_path = fixture.checkout.join("source.rs");
    fs::write(&manifest_path, &manifest).expect("declare formatter gate");
    fs::write(fixture.checkout.join("VERSION"), env!("CARGO_PKG_VERSION"))
        .expect("write declared version");
    fs::write(&source_path, "pub fn measured() -> bool { true }\n")
        .expect("write real Rust source");
    fixture.success(
        Command::new("rustfmt")
            .arg("--edition")
            .arg("2021")
            .arg(&source_path),
    );
    let formatted = fs::read(&source_path).expect("read formatter output");
    let good = fixture.commit();
    fs::write(&source_path, "pub fn measured()->bool{true}\n").expect("write unformatted source");
    let bad = fixture.commit();
    assert_ne!(good, bad, "the two committed trees must differ");
    // HEAD now fails its real formatter, while the uncommitted source rescues it
    // and the uncommitted manifest disables releases. Neither may affect a push.
    fs::write(&source_path, &formatted).expect("leave another writer's repaired source");
    fs::write(&manifest_path, serde_json::to_vec(&json!({
        "schema_version": own["schema_version"], "product": "quality-push-fixture", "releases": false
    })).expect("encode uncommitted declaration")).expect("leave unrelated dirty declaration");
    let dirty_manifest = fs::read(&manifest_path).expect("snapshot dirty declaration");
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
        transcript.contains(&bad),
        "refusal did not identify its committed source: {transcript}"
    );
    assert_eq!(
        fixture.published(),
        good,
        "refused push changed remote state"
    );
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

    // A real blob exists but is not a commit: explicit revision selection
    // refuses it instead of quietly choosing HEAD or another source tree.
    let blob = fixture.success(
        fixture
            .git()
            .arg("rev-parse")
            .arg(format!("{good}:source.rs")),
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
        formatted
    );
    assert_eq!(
        fs::read(&manifest_path).expect("read working declaration"),
        dirty_manifest
    );
    fixture.passed();
}
