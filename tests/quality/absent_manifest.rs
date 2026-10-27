//! A committed checkout without `.wisent-release.json` declares no quality
//! gate: `stado quality check --if-declared`, and the pre-push check a Git
//! dispatcher runs with it, answer that with exit 0, while a check without
//! `--if-declared` refuses with the same sentence, and a manifest that is
//! present but cannot be read is still refused.
//!
//! Each case is a real Git repository with one commit, made under Cargo's
//! test directory with an isolated `HOME`, read by the real `stado`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// What the check says of a checkout that holds no manifest.
const ABSENT: &str = "is absent, so it declares no quality gate";

struct Checkout {
    root: PathBuf,
    home: PathBuf,
}

impl Checkout {
    /// A repository whose one commit holds `files`.
    fn committed(case: &str, files: &[(&str, &str)]) -> Self {
        let base = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.build/journeys/quality-absent-manifest")
            .join(format!("{case}-{}", std::process::id()));
        let root = base.join("checkout");
        let home = base.join("home");
        std::fs::create_dir_all(&root).expect("create the checkout");
        std::fs::create_dir_all(&home).expect("create the isolated home");
        let checkout = Self { root, home };
        checkout.git(&["init", "--quiet", "--initial-branch", "main"]);
        for (path, text) in files {
            let file = checkout.root.join(path);
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent).expect("create the file's directory");
            }
            std::fs::write(file, text).expect("write the committed file");
        }
        checkout.git(&["add", "--all"]);
        checkout.git(&["commit", "--quiet", "-m", "A committed tree"]);
        checkout
    }

    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(&self.root)
            .env("GIT_AUTHOR_NAME", "Quality Journey")
            .env("GIT_AUTHOR_EMAIL", "quality-journey@example.invalid")
            .env("GIT_COMMITTER_NAME", "Quality Journey")
            .env("GIT_COMMITTER_EMAIL", "quality-journey@example.invalid")
            .args(args)
            .output()
            .expect("run git");
        assert!(output.status.success(), "git {args:?}: {output:?}");
        String::from_utf8(output.stdout)
            .expect("git output is UTF-8")
            .trim()
            .to_owned()
    }

    fn quality(&self, args: &[&str], stdin: &str) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_stado"))
            .env("HOME", &self.home)
            .env("STADO_CONFIG", self.home.join("no-config.toml"))
            .args(["quality", "check", "--root"])
            .arg(&self.root)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run the real stado");
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(stdin.as_bytes())
            .expect("write the pre-push updates");
        child.wait_with_output().expect("stado ends")
    }
}

impl Drop for Checkout {
    fn drop(&mut self) {
        if let Some(base) = self.root.parent() {
            let _ = std::fs::remove_dir_all(base);
        }
    }
}

fn said(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn a_checkout_without_a_manifest_declares_no_gate() {
    let checkout =
        Checkout::committed("absent", &[("README.md", "A checkout with no manifest.\n")]);
    let head = checkout.git(&["rev-parse", "HEAD"]);

    let declared = checkout.quality(&["--revision", &head, "--if-declared"], "");
    assert!(
        declared.status.success(),
        "--if-declared: {}",
        said(&declared)
    );
    assert!(
        said(&declared).contains(&format!("{ABSENT}; nothing to check")),
        "--if-declared names the absent manifest: {}",
        said(&declared)
    );

    let updates = format!(
        "refs/heads/main {head} refs/heads/main {}\n",
        "0".repeat(head.len())
    );
    let pushed = checkout.quality(&["--pre-push", "--if-declared"], &updates);
    assert!(pushed.status.success(), "pre-push: {}", said(&pushed));
    assert!(
        said(&pushed).contains(&format!("{ABSENT}; nothing to check")),
        "the pre-push check names the absent manifest: {}",
        said(&pushed)
    );

    let explicit = checkout.quality(&["--revision", &head], "");
    assert!(
        !explicit.status.success(),
        "a check without --if-declared passed"
    );
    assert!(
        said(&explicit).contains(ABSENT),
        "the refusal names the absent manifest: {}",
        said(&explicit)
    );
}

#[test]
fn a_manifest_that_cannot_be_read_is_refused() {
    let checkout = Checkout::committed(
        "unreadable",
        &[(
            ".wisent-release.json/README.md",
            "A directory where the manifest belongs.\n",
        )],
    );
    let head = checkout.git(&["rev-parse", "HEAD"]);
    let refused = checkout.quality(&["--revision", &head, "--if-declared"], "");
    assert!(
        !refused.status.success(),
        "an unreadable manifest passed: {}",
        said(&refused)
    );
    assert!(
        said(&refused).contains("cannot read") && !said(&refused).contains(ABSENT),
        "the refusal names the read that failed, not an absent manifest: {}",
        said(&refused)
    );
}
