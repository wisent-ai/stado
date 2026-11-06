//! The staged activation's own script, run on this machine against a home of
//! the test's own: an archive whose installer moves `$HOME/weles` to the new
//! release and writes to its log. The script is the one `stado release staged
//! activate` sends to a host (`activation_script`), run by the same
//! `/bin/bash -s` the host channel runs it with; no host is contacted and no
//! operator state is read or written.
//!
//! Each case checks what the report says about the installer's log (only the
//! bytes this run appended; the whole log when it was created or replaced,
//! including by a longer file) and that the link is read again only after the
//! operator's settle window.

use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use stado::deploy::staged_release::{activation_script, INSTALLER_MEMBER};

/// The window the operator states; the smallest the argument admits, so the
/// journey waits as little as the contract allows.
const SETTLE: NonZeroU64 = NonZeroU64::MIN;

struct Home {
    root: PathBuf,
}

impl Home {
    fn new(name: &str) -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("staged-activation-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home/.local/state/weles")).expect("create the log directory");
        std::os::unix::fs::symlink("releases/old", root.join("home/weles")).expect("link the old release");
        Self { root }
    }

    fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    fn log(&self) -> PathBuf {
        self.home().join(".local/state/weles/auto-deploy.log")
    }

    /// A release archive whose installer is `installer`, at the member path
    /// the activation extracts.
    fn archive(&self, installer: &str) -> PathBuf {
        let tree = self.root.join("archive-tree");
        let member = tree.join(INSTALLER_MEMBER);
        std::fs::create_dir_all(member.parent().expect("the member has a directory"))
            .expect("create the member's directory");
        std::fs::write(&member, installer).expect("write the installer");
        let archive = self.root.join("release.tar.gz");
        let packed = Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&tree)
            .arg(".")
            .status()
            .expect("tar runs");
        assert!(packed.success(), "tar packed the archive: {packed}");
        archive
    }

    /// Run the activation script for `archive` the way the host channel does,
    /// and answer its standard output and how long it took.
    fn activate(&self, archive: &Path) -> (String, Duration) {
        let script = activation_script(&archive.display().to_string(), "1.0.0", SETTLE);
        let started = Instant::now();
        let mut child = Command::new("/bin/bash")
            .arg("-s")
            .env("HOME", self.home())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("bash runs");
        std::io::Write::write_all(
            child.stdin.as_mut().expect("bash's stdin is piped"),
            script.as_bytes(),
        )
        .expect("the script is written to bash");
        let output = child.wait_with_output().expect("the activation ends");
        let took = started.elapsed();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(
            output.status.success(),
            "the activation exited {}; stdout: {stdout}; stderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        (stdout, took)
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The installer every case shares: it moves the link to the new release, as
/// the real one does, and then runs `log_step`.
fn installer(log_step: &str) -> String {
    format!(
        "set -eu\nln -sfn releases/new \"$HOME/weles\"\n{log_step}\n"
    )
}

fn log_lines(report: &str) -> Vec<&str> {
    report
        .lines()
        .filter_map(|line| line.strip_prefix("STADO_ACTIVATE_LOG "))
        .collect()
}

#[test]
fn only_what_this_run_appended_is_reported_and_the_link_is_read_after_the_window() {
    let home = Home::new("appended");
    std::fs::write(home.log(), "an earlier run's line\n").expect("write the earlier log");
    let archive = home.archive(&installer(
        "printf 'installed 1.0.0\\n' >> \"$HOME/.local/state/weles/auto-deploy.log\"",
    ));
    let (report, took) = home.activate(&archive);
    assert_eq!(log_lines(&report), ["installed 1.0.0"], "report: {report}");
    assert!(report.contains("STADO_ACTIVATE_LINK releases/new"), "report: {report}");
    assert!(report.contains("STADO_ACTIVATE_SETTLED releases/new"), "report: {report}");
    assert!(
        took >= Duration::from_secs(SETTLE.get()),
        "the link was read again after {took:?}, before the {SETTLE} s window"
    );
}

#[test]
fn a_log_replaced_by_a_longer_file_is_reported_whole() {
    let home = Home::new("replaced");
    std::fs::write(home.log(), "old\n").expect("write the earlier log");
    let archive = home.archive(&installer(
        "printf 'a new log, longer than the one it replaced\\n' > \"$HOME/.local/state/weles/auto-deploy.log.new\"\n\
         mv \"$HOME/.local/state/weles/auto-deploy.log.new\" \"$HOME/.local/state/weles/auto-deploy.log\"",
    ));
    let (report, _) = home.activate(&archive);
    let lines = log_lines(&report);
    assert!(
        lines.first().is_some_and(|line| line.ends_with("was replaced during the run; all of it follows")),
        "report: {report}"
    );
    assert_eq!(
        lines.get(1..),
        Some(&["a new log, longer than the one it replaced"][..]),
        "report: {report}"
    );
}

#[test]
fn a_log_the_run_created_is_reported_whole() {
    let home = Home::new("created");
    let archive = home.archive(&installer(
        "printf 'first line\\n' > \"$HOME/.local/state/weles/auto-deploy.log\"",
    ));
    let (report, _) = home.activate(&archive);
    let lines = log_lines(&report);
    assert!(
        lines.first().is_some_and(|line| line.ends_with("was created during the run; all of it follows")),
        "report: {report}"
    );
    assert_eq!(lines.get(1..), Some(&["first line"][..]), "report: {report}");
}

#[test]
fn a_run_that_wrote_nothing_says_so() {
    let home = Home::new("silent");
    std::fs::write(home.log(), "an earlier run's line\n").expect("write the earlier log");
    let archive = home.archive(&installer(":"));
    let (report, _) = home.activate(&archive);
    let lines = log_lines(&report);
    assert_eq!(lines.len(), 1, "report: {report}");
    assert!(
        lines[0].starts_with("the installer wrote nothing to "),
        "report: {report}"
    );
}

#[test]
fn a_run_that_left_no_log_says_so() {
    let home = Home::new("absent");
    let archive = home.archive(&installer(":"));
    let (report, _) = home.activate(&archive);
    let lines = log_lines(&report);
    assert_eq!(lines.len(), 1, "report: {report}");
    assert!(lines[0].starts_with("the installer wrote no log at "), "report: {report}");
}
