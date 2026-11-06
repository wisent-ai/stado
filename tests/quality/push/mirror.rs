//! The remote Git hands the pre-push hook decides whether a push is checked.
//! A second remote, a mirror: the commit origin refused, pushed to the
//! mirror's main, reaches no repository origin names, so it is not checked,
//! the check says where it went, and the mirror receives it. Origin's own
//! repository named by its URL (no remote name, a trailing `/`) is the same
//! repository, so the same commit is still refused there; and a push Git
//! names as origin is checked even when its URL reads like the mirror's.

use std::process::{Command, Output};

use super::fixture::Fixture;

/// What a run printed, both streams.
fn said(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// `good` is published on origin and `bad` was refused there.
pub fn pushed_elsewhere(fixture: &mut Fixture, good: &str, bad: &str) {
    let mirror = fixture.root.join("mirror.git");
    fixture.success(
        Command::new("git")
            .arg("init")
            .arg("--quiet")
            .arg("--bare")
            .arg(&mirror),
    );
    fixture.success(
        fixture
            .git()
            .arg("remote")
            .arg("add")
            .arg("mirror")
            .arg(&mirror),
    );
    let mirrored = fixture.success(
        fixture
            .git()
            .arg("push")
            .arg("mirror")
            .arg(format!("{bad}:refs/heads/main")),
    );
    let transcript = said(&mirrored);
    assert!(
        transcript.contains("this push goes to mirror")
            && transcript.contains("so it is not checked")
            && !transcript.contains("checking the committed source"),
        "a push to a mirror was checked as one to origin: {transcript}"
    );
    let on_mirror = fixture.success(
        Command::new("git")
            .arg("--git-dir")
            .arg(&mirror)
            .arg("rev-parse")
            .arg("refs/heads/main"),
    );
    assert_eq!(
        String::from_utf8_lossy(&on_mirror.stdout).trim(),
        bad,
        "the mirror did not receive the push"
    );

    let by_url = format!("{}/", fixture.remote.display());
    let refused = fixture.run(
        fixture
            .git()
            .arg("push")
            .arg(&by_url)
            .arg(format!("{bad}:refs/heads/main")),
    );
    let transcript = said(&refused);
    assert!(
        !refused.status.success() && transcript.contains("missing"),
        "a push to origin's repository named by its URL was not checked: {transcript}"
    );
    assert_eq!(
        fixture.published(),
        good,
        "refused push changed remote state"
    );

    // A push Git names as origin is checked whatever its URL reads: Git
    // hands the hook the URL after its own rewrites (an insteadOf alias, a
    // forge's second SSH host), which can spell origin's repository in a way
    // origin's configured URLs do not. Fed Git's update with origin's name
    // and the mirror's URL, the check still checks the bad commit.
    let mut check = fixture.stado();
    check
        .args([
            "quality",
            "check",
            "--pre-push",
            "--if-declared",
            "--remote",
            "origin",
        ])
        .arg("--root")
        .arg(&fixture.checkout)
        .arg("--remote-url")
        .arg(&mirror)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = check.spawn().expect("the real Stado starts");
    let update = format!("refs/heads/main {bad} refs/heads/main {good}\n");
    std::io::Write::write_all(
        child.stdin.as_mut().expect("the check reads stdin"),
        update.as_bytes(),
    )
    .expect("Git's update is handed over");
    let checked = child.wait_with_output().expect("the check ends");
    let transcript = said(&checked);
    assert!(
        !checked.status.success()
            && transcript.contains("missing")
            && !transcript.contains("so it is not checked"),
        "a push Git named as origin was skipped for its URL: {transcript}"
    );
}
