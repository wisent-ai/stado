//! Real Git, the built CLI and the real local object store. Run in the batch.
use super::area::{releasing_manifest, Area};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}
fn document(output: std::process::Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("CLI JSON")
}

#[test]
fn pushed_commits_wait_together_without_starting_a_build() {
    let area = Area::new("release-changes");
    let revision = std::env::var("WISENT_SOURCE_COMMIT").unwrap_or_else(|_| {
        git(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            &["rev-parse", "HEAD"],
        )
    });
    std::fs::write(area.root.join("source-revision"), revision).unwrap();
    let mut manifest: Value = serde_json::from_str(&releasing_manifest("pending-product")).unwrap();
    let product: Value =
        serde_json::from_str(include_str!("../../../.wisent-release.json")).unwrap();
    manifest["platforms"] = product["platforms"].clone();
    let root = area.checkout(
        "pending-product",
        &manifest.to_string(),
        Some(("package.json", "{\"version\":\"1.0.0\"}")),
    );
    let remote = area.root.join("origin.git");
    std::fs::create_dir(&remote).unwrap();
    git(&remote, &["init", "--bare", "--initial-branch=main"]);
    git(
        &root,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&root, &["push", "origin", "main"]);
    let first = git(&root, &["rev-parse", "HEAD"]);
    std::fs::write(root.join("feature.txt"), "second change\n").unwrap();
    git(&root, &["add", "feature.txt"]);
    git(&root, &["commit", "-m", "second source change"]);
    let second = git(&root, &["rev-parse", "HEAD"]);
    git(&root, &["push", "origin", "main"]);
    let task = format!("task-{}", &first[..16]);
    let submit = |revision: &str| {
        area.stado(&[
            "release",
            "changes",
            "submit",
            "--source",
            root.to_str().unwrap(),
            "--commit",
            revision,
            "--task",
            &task,
            "--session",
            "release-change-journey",
            "--json",
        ])
    };
    let a = document(submit(&first));
    let b = document(submit(&second));
    assert_eq!(a["state"], "queued");
    assert_eq!(b["state"], "queued");
    assert_ne!(a["id"], b["id"]);
    assert_eq!(
        document(submit(&first))["id"],
        a["id"],
        "retry is the same immutable handoff"
    );
    let rows = document(area.stado(&["release", "changes", "list", "--task", &task, "--json"]));
    let rows = rows.as_array().unwrap();
    assert_eq!(
        rows.len(),
        2,
        "both pushed changes persist across CLI processes"
    );
    for row in rows {
        assert_eq!(row["state"], "queued");
        assert!(row["run_id"].is_null());
    }
    std::fs::write(root.join("feature.txt"), "unpushed change\n").unwrap();
    git(&root, &["add", "feature.txt"]);
    git(&root, &["commit", "-m", "not pushed"]);
    let unpushed = git(&root, &["rev-parse", "HEAD"]);
    let refused = submit(&unpushed);
    assert!(!refused.status.success());
    let after = document(area.stado(&["release", "changes", "list", "--task", &task, "--json"]));
    assert_eq!(
        after.as_array().unwrap().len(),
        2,
        "refused source was not queued"
    );

    // A later standalone build, not a release, covers both ancestor commits.
    // This isolated store cannot admit a fleet build. Its real refusal must
    // fail the covered changes rather than leave them queued forever.
    let refused_build = area.stado(&[
        "build",
        "submit",
        "--source",
        root.to_str().unwrap(),
        "--commit",
        &second,
        "--version",
        "1.0.0",
        "--json",
    ]);
    assert!(!refused_build.status.success());
    let verdicts = document(area.stado(&["release", "changes", "list", "--task", &task, "--json"]));
    let verdicts = verdicts.as_array().unwrap();
    assert_eq!(verdicts.len(), 2);
    for verdict in verdicts {
        assert_eq!(verdict["state"], "failed");
        assert_eq!(verdict["task_id"], task);
        let build = document(area.stado(&[
            "build",
            "status",
            verdict["run_id"].as_str().unwrap(),
            "--json",
        ]));
        assert_eq!(build["state"], "failed");
        assert_eq!(build["source_commit"], second);
    }
    assert_eq!(verdicts[0]["run_id"], verdicts[1]["run_id"]);

    // Names must remain unique across source quality and post-build tests. The
    // product manifest this copies no longer declares post-build tests, so the
    // test step is written here with the first quality step's name.
    let recipe = &mut manifest["platforms"]["darwin-arm64"];
    let name = recipe["quality"][0]["name"].clone();
    recipe["tests"] = serde_json::json!([{ "name": name, "argv": ["true"] }]);
    std::fs::write(root.join(".wisent-release.json"), manifest.to_string()).unwrap();
    git(&root, &["add", ".wisent-release.json"]);
    git(&root, &["commit", "-m", "duplicate pipeline step name"]);
    git(&root, &["push", "origin", "main"]);
    let duplicate = git(&root, &["rev-parse", "HEAD"]);
    assert!(!submit(&duplicate).status.success());
    let unchanged =
        document(area.stado(&["release", "changes", "list", "--task", &task, "--json"]));
    assert_eq!(
        unchanged.as_array().unwrap().len(),
        verdicts.len(),
        "a refused manifest must not record a handoff"
    );
}
