use super::*;

#[test]
fn a_fetch_failure_and_an_unresolved_lock_are_told_apart_and_a_current_lock_passes() {
    let mut deployment = Deployment::start();
    let initialized = deployment.run(&["config", "init"]);
    let said = String::from_utf8_lossy(&initialized.stderr).into_owned();
    assert!(initialized.status.success(), "stado config init: {said}");
    let crate_manifest =
        format!("[package]\nname = \"{DEPENDENCY}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
    let dependency = deployment.repository(
        DEPENDENCY,
        &[
            ("Cargo.toml", crate_manifest),
            (
                "src/lib.rs",
                "pub const FETCHED: bool = true;\n".to_string(),
            ),
        ],
        false,
    );
    let commit = git_output(&dependency, &["rev-parse", "HEAD"]);
    let origin = deployment
        .root
        .join("origins")
        .join(format!("{DEPENDENCY}.git"));
    let url = format!("file://{}", origin.display());

    // Current: the Git dependency is fetched and the committed lock resolves.
    let current = deployment.checkout("current", &url, &commit);
    let checked = deployment.run(&["quality", "check", "--root", &current.to_string_lossy()]);
    let said = String::from_utf8_lossy(&checked.stderr).into_owned();
    assert!(
        checked.status.success(),
        "a current lock did not pass: {said}"
    );
    deployment.report["current"] = json!(said.trim());

    // Stale: the manifest names a dependency the committed lock lacks.
    let stale = deployment.checkout("stale", &url, &commit);
    Deployment::outdate_lock(&stale);
    let refused = deployment.run(&["quality", "check", "--root", &stale.to_string_lossy()]);
    let said = String::from_utf8_lossy(&refused.stderr).into_owned();
    assert!(!refused.status.success(), "a stale lock passed: {said}");
    assert!(
        said.contains("cargo metadata --locked --offline --format-version 1")
            && !said.contains("cargo fetch --manifest-path Cargo.toml failed"),
        "the failed operation is metadata after a successful fetch: {said}"
    );
    deployment.report["stale"] = json!(said.trim());

    // A pin held only by the sibling checkout adds local evidence to Cargo's fetch cause.
    let local_pin = deployment.checkout("local-pin", &url, &commit);
    fs::write(
        dependency.join("src/lib.rs"),
        "pub const FETCHED: bool = false;\n",
    )
    .unwrap();
    git(&dependency, &["add", "src/lib.rs"]);
    git(
        &dependency,
        &["commit", "--quiet", "-m", "local source revision"],
    );
    let unpublished = git_output(&dependency, &["rev-parse", "HEAD"]);
    let manifest = local_pin.join("Cargo.toml");
    let text = fs::read_to_string(&manifest)
        .unwrap()
        .replace(&commit, &unpublished);
    fs::write(&manifest, text).unwrap();
    git(&local_pin, &["add", "Cargo.toml"]);
    git(
        &local_pin,
        &["commit", "--quiet", "-m", "pin local revision"],
    );
    git(&local_pin, &["push", "--quiet", "origin", "main"]);
    let failed = deployment.run(&["quality", "check", "--root", &local_pin.to_string_lossy()]);
    let said = String::from_utf8_lossy(&failed.stderr).into_owned();
    assert!(!failed.status.success(), "a local-only pin passed: {said}");
    assert!(
        said.contains("cargo fetch --manifest-path Cargo.toml failed")
            && said.contains(&unpublished)
            && said.contains("no remote-tracking branch")
            && said.contains("whether its Cargo.lock resolves was not asked"),
        "the refusal preserves the failed fetch and observed local pin: {said}"
    );
    deployment.report["local_pin"] = json!(said.trim());

    // Unfetchable: the dependency's origin is gone after the lock was written.
    let unfetchable = deployment.checkout("unfetchable", &url, &commit);
    fs::rename(&origin, deployment.root.join("moved-origin.git")).unwrap();
    let failed = deployment.run(&["quality", "check", "--root", &unfetchable.to_string_lossy()]);
    let said = String::from_utf8_lossy(&failed.stderr).into_owned();
    assert!(
        !failed.status.success(),
        "an unfetchable source passed: {said}"
    );
    assert!(
        said.contains("cargo fetch --manifest-path Cargo.toml failed")
            && said.contains(DEPENDENCY)
            && !said.contains("cargo metadata --locked --offline --format-version 1"),
        "the fetch failure names the dependency without claiming a metadata verdict: {said}"
    );
    deployment.report["unfetchable"] = json!(said.trim());

    deployment.report["outcome"] = json!("passed");
    deployment.save();
    eprintln!("report: {}", deployment.root.join("report.json").display());
}
