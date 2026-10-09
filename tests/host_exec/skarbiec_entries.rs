//! `stado host exec TARGET -- skarbiec doctor` and `-- skarbiec
//! recover-daemons` pass the allowlist, and an unapproved Skarbiec spelling
//! is refused naming both.
//!
//! The real `stado` runs against an isolated local store under the build
//! directory (`WC_STORAGE_BACKEND=local`, `WC_LOCAL_STORAGE_PATH`, a
//! `STADO_CONFIG` naming no file) whose registry knows no host. An
//! unapproved spelling is refused before the registry is read, with the
//! approved spellings; an approved one is admitted and the command goes on
//! to resolve the target, which the empty registry refuses by name — so the
//! allowlist's answer and the registry's are told apart by the sentence.
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const TARGET: &str = "example-host";
const UNAPPROVED: &str = "'skarbiec status' is not an approved host-exec command";
const REFUSAL: &str = "is not an approved host-exec command";
const APPROVED_PREFIX: &str = "approved commands: ";
const DOCTOR: &str = "skarbiec doctor";
const RECOVER_DAEMONS: &str = "skarbiec recover-daemons";

struct Store {
    root: PathBuf,
}

impl Store {
    fn new(case: &str) -> Self {
        let parent = Path::new(env!("CARGO_TARGET_TMPDIR")).join("host-exec-skarbiec");
        std::fs::create_dir_all(&parent).expect("create the test output directory");
        let root = parent.join(format!("{case}-{}", std::process::id()));
        for directory in [".locks", ".metadata"] {
            std::fs::create_dir_all(root.join("store").join(directory))
                .expect("lay out the isolated local store");
        }
        Self { root }
    }

    /// Run `stado host exec TARGET -- <spelling>`, the spelling split into
    /// its words the way an operator types them.
    fn run(&self, spelling: &str) -> Output {
        let mut arguments = vec!["host", "exec", TARGET, "--"];
        arguments.extend(spelling.split(' '));
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(&arguments)
            .current_dir(&self.root)
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .env("STADO_CONFIG", self.root.join("no-config.toml"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("run the real stado");
        std::fs::write(
            self.root
                .join(format!("{}.stderr", spelling.replace(' ', "-"))),
            &output.stderr,
        )
        .expect("keep the command's stderr beside the store");
        output
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn an_unapproved_skarbiec_spelling_is_refused_naming_the_approved_ones() {
    let store = Store::new("unapproved");
    let output = store.run("skarbiec status");
    let said = stderr(&output);
    assert!(
        !output.status.success(),
        "an unapproved spelling is refused: {said}"
    );
    assert!(
        said.contains(UNAPPROVED),
        "the refusal names the spelling it refused: {said}"
    );
    let approved = match said
        .lines()
        .find_map(|line| line.split_once(APPROVED_PREFIX).map(|(_, rest)| rest))
    {
        Some(approved) => approved,
        None => panic!("the refusal carries the approved spellings: {said}"),
    };
    let spellings: Vec<&str> = approved.split(", ").map(str::trim).collect();
    assert!(
        spellings.contains(&DOCTOR),
        "{DOCTOR} is an approved spelling: {approved}"
    );
    assert!(
        spellings.contains(&RECOVER_DAEMONS),
        "{RECOVER_DAEMONS} is an approved spelling: {approved}"
    );
}

/// An admitted spelling goes on past the allowlist to resolve its target,
/// which this store's empty registry cannot: the command still fails, but
/// not with the allowlist's refusal.
fn admitted(store: &Store, spelling: &str) {
    let output = store.run(spelling);
    let said = stderr(&output);
    assert!(
        !said.contains(REFUSAL),
        "{spelling} is admitted by the allowlist: {said}"
    );
    assert!(
        !output.status.success(),
        "{spelling} went on to the registry, which this isolated store cannot answer: {said}"
    );
}

#[test]
fn skarbiec_doctor_passes_the_allowlist() {
    admitted(&Store::new("doctor"), DOCTOR);
}

#[test]
fn skarbiec_recover_daemons_passes_the_allowlist() {
    admitted(&Store::new("recover-daemons"), RECOVER_DAEMONS);
}
