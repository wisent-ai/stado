//! `stado credentials try-unlock` stops as unreachable, naming Skarbiec's
//! exit status, the parse error, the transcript record the phrase came from
//! and Skarbiec's own stderr, when `skarbiec key-doctor` answers without a
//! JSON report; it does not skip the run and conclude that no transcript
//! phrase opens the vault.
//!
//! The real stado runs with an isolated `HOME` under the checkout's build
//! directory holding one agent transcript that records an unlock phrase, and
//! the real Skarbiec named by `SKARBIEC_BIN` (the Skarbiec under test, as the
//! credential scripts name it), whose `key-doctor` in that `HOME` finds no
//! initialised vault and answers an error instead of a report. No
//! `SKARBIEC_BIN` is a failed run, never a passed one.
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::json;

const RECORD: &str = "SKARBIEC_UNLOCK_PHRASE";
const PHRASE: &str = "transcript-recorded-phrase-for-this-test";
const WITHOUT_REPORT: &str = "without a JSON report";
const NOT_IN_TRANSCRIPTS: &str = "is not in any transcript";

struct Run {
    root: PathBuf,
    skarbiec: PathBuf,
}

impl Run {
    fn new() -> Self {
        let skarbiec = std::env::var_os("SKARBIEC_BIN")
            .map(PathBuf::from)
            .filter(|path| path.is_file())
            .expect("SKARBIEC_BIN names the Skarbiec under test, a file");
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the package sits inside the checkout")
            .to_path_buf();
        let root = repository
            .join(".build/credentials-unlock-doctor-without-report")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        Self { root, skarbiec }
    }

    /// One agent transcript whose tool result records the phrase under a
    /// name the recovery query reads as unlock material.
    fn write_transcript(&self) {
        let sessions = self.root.join(".omp/agent/sessions/example");
        std::fs::create_dir_all(&sessions).unwrap();
        let event = json!({
            "type": "message",
            "message": {
                "role": "toolResult",
                "toolName": "bash",
                "content": [{"type": "text", "text": format!("{RECORD}={PHRASE}\n")}],
            },
        });
        std::fs::write(
            sessions.join("session.jsonl"),
            format!("{}\n", serde_json::to_string(&event).unwrap()),
        )
        .unwrap();
    }

    fn try_unlock(&self) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(["credentials", "try-unlock"])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").expect("PATH names the tools"))
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join("no-config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("SKARBIEC_BIN", &self.skarbiec)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .expect("run the real stado");
        std::fs::write(self.root.join("try-unlock.stdout"), &output.stdout).unwrap();
        std::fs::write(self.root.join("try-unlock.stderr"), &output.stderr).unwrap();
        output
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn a_key_doctor_that_answers_no_report_stops_the_run_naming_it_not_the_phrases() {
    let run = Run::new();
    run.write_transcript();
    let output = run.try_unlock();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert!(
        !output.status.success(),
        "a run whose oracle answered nothing is not a verdict: {stdout}{stderr}"
    );
    assert!(
        stderr.contains(WITHOUT_REPORT) && stderr.contains(RECORD),
        "the refusal says key-doctor answered without a report while testing the phrase recorded under {RECORD}: {stderr}"
    );
    assert!(
        stderr.contains("vault not initialized"),
        "Skarbiec's own stderr is carried in the refusal: {stderr}"
    );
    assert!(
        !stderr.contains(NOT_IN_TRANSCRIPTS),
        "a failing oracle is not reported as a passphrase missing from every transcript: {stderr}"
    );
    assert!(
        !stdout.contains(PHRASE) && !stderr.contains(PHRASE),
        "the phrase itself is never printed: {stdout}{stderr}"
    );
}
