//! `stado workdirs`, driven as the real binary against an isolated home.
//!
//! The command exists because a root every agent is told to write to,
//! `~/.stado/work`, had no cleaner behind it and grew to 354 GiB on this Mac
//! while the janitor reported a healthy host. What makes it safe to run again
//! is not that it deletes, but what it refuses to delete, so that is what the
//! assertions read off the filesystem: the queue's own area, the host-run
//! area, a loose file, and a symlink whose target lives outside the root.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Directory the assertions treat as another owner's, matching
/// `providers::local::scratch_workdirs::DECLARED_AREAS`.
const QUEUE_AREA: &str = "jobs";
const RUN_AREA: &str = "runs";

struct Fixture {
    home: PathBuf,
    outside: PathBuf,
}

impl Fixture {
    /// A home with two disposable working directories, both declared areas, a
    /// loose file, and a symlink pointing outside the root.
    fn new(label: &str) -> Self {
        let base = std::env::temp_dir().join(format!(
            "stado-workdirs-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_nanos())
                .unwrap_or_default()
        ));
        let home = base.join("home");
        let outside = base.join("outside");
        let work = home.join(".stado/work");
        fs::create_dir_all(work.join("alpha/nested")).expect("create alpha");
        fs::create_dir_all(work.join("beta")).expect("create beta");
        fs::create_dir_all(work.join(QUEUE_AREA)).expect("create queue area");
        fs::create_dir_all(work.join(RUN_AREA)).expect("create run area");
        fs::create_dir_all(&outside).expect("create outside");
        fs::write(work.join("alpha/nested/big.bin"), vec![b'x'; 4096]).expect("write payload");
        fs::write(work.join("corpus.jsonl"), b"{}\n").expect("write loose file");
        fs::write(outside.join("keep.txt"), b"keep\n").expect("write outside payload");
        std::os::unix::fs::symlink(&outside, work.join("escape")).expect("create symlink");
        Self { home, outside }
    }

    fn work(&self) -> PathBuf {
        self.home.join(".stado/work")
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env("HOME", &self.home)
            .env("STADO_CONFIG", self.home.join("no-such-config.json"))
            .output()
            .expect("run stado workdirs")
    }

    fn cleanup(&self) {
        if let Some(base) = self.home.parent() {
            let _ = fs::remove_dir_all(base);
        }
    }
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

#[test]
fn reporting_changes_nothing_and_applying_removes_only_undeclared_directories() {
    let fixture = Fixture::new("sweep");
    let work = fixture.work();

    let plan = fixture.run(&["workdirs"]);
    let planned = String::from_utf8_lossy(&plan.stdout).into_owned();
    assert_eq!(
        plan.status.code(),
        Some(0),
        "reporting succeeds: {}",
        String::from_utf8_lossy(&plan.stderr)
    );
    assert!(
        planned.contains("pass --apply to remove them"),
        "the report says how to act on it: {planned}"
    );
    assert!(
        exists(&work.join("alpha")) && exists(&work.join("beta")),
        "reporting removed nothing"
    );

    let applied = fixture.run(&["workdirs", "--apply"]);
    let text = String::from_utf8_lossy(&applied.stdout).into_owned();
    assert_eq!(
        applied.status.code(),
        Some(0),
        "the sweep succeeds: {}",
        String::from_utf8_lossy(&applied.stderr)
    );

    assert!(!exists(&work.join("alpha")), "alpha was removed: {text}");
    assert!(!exists(&work.join("beta")), "beta was removed: {text}");
    assert!(
        exists(&work.join(QUEUE_AREA)),
        "the queue's own area is left to the queue_workdirs cleaner: {text}"
    );
    assert!(
        exists(&work.join(RUN_AREA)),
        "the host-run area is left to deploy::host_run: {text}"
    );
    assert!(
        exists(&work.join("corpus.jsonl")),
        "a loose file is not a working directory: {text}"
    );
    assert!(
        exists(&work.join("escape")),
        "the symlink itself is left in place: {text}"
    );
    assert!(
        exists(&fixture.outside.join("keep.txt")),
        "the symlink was never followed out of the root: {text}"
    );
    fixture.cleanup();
}

#[test]
fn a_root_that_does_not_exist_is_reported_rather_than_failing() {
    let fixture = Fixture::new("absent");
    fs::remove_dir_all(fixture.work()).expect("remove the root");

    let output = fixture.run(&["workdirs"]);
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    assert_eq!(
        output.status.code(),
        Some(0),
        "an absent root is not an error: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        text.contains("does not exist"),
        "the report names the missing root: {text}"
    );
    fixture.cleanup();
}
