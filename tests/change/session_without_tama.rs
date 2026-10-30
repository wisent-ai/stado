//! Run with `cargo test --manifest-path stado-rs/Cargo.toml --test change-session-without-tama -- --nocapture`.
//!
//! Whose work a push, a submit or a queue pass is reaches Oko's admission
//! (`oko admit … --session`) even on a machine with no Tama installed: the
//! session is then the one the harness running the command names in
//! `PI_SESSION_FILE`, read from that transcript's session record. The
//! operator's consent held to one session, and its subagents, reached no
//! push of theirs while Tama was uninstalled, because no session was named
//! and his older withdrawal refused every build.
//!
//! The real `stado change run` with no checkout, in a home of the run's own
//! with no Tama under it and an empty queue, says whether it runs as an
//! agent session's work: with `PI_SESSION_FILE` naming an OMP transcript it
//! names that session; with none it is the resident pass, as before; with a
//! variable naming a file that is gone, or a file that holds no session
//! record, it says why the session could not be read and asks as no one.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The session the fixture transcript names, as OMP writes its id.
const SESSION: &str = "01a10000-0000-7000-8000-00000000c0de";

struct Run {
    root: PathBuf,
}

impl Run {
    fn new() -> Self {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let root = repository
            .join(".build/change-session-without-tama")
            .join(uuid::Uuid::new_v4().to_string());
        for directory in ["home/.stado", "queue", "tmp", "workspace"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        Self { root }
    }

    /// A transcript as OMP writes one: a title record, then the session.
    fn transcript(&self) -> PathBuf {
        let path = self.root.join("session.jsonl");
        let lines = [
            r#"{"type":"title","v":1,"title":""}"#.to_string(),
            format!(
                r#"{{"type":"session","version":3,"id":"{SESSION}","timestamp":"2026-10-30T03:29:13.822Z"}}"#
            ),
        ];
        fs::write(&path, lines.join("\n") + "\n").unwrap();
        path
    }

    /// `stado change run` with no checkout, `PI_SESSION_FILE` set to `named`
    /// or unset.
    fn run(&self, named: Option<&Path>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .args(["change", "run"])
            .env("HOME", self.root.join("home"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("STADO_CONFIG", self.root.join("home/.stado/config.json"))
            .env("STADO_CHANGE_QUEUE", self.root.join("queue"))
            .env("WISENT_WORKSPACE", self.root.join("workspace"))
            .env_remove("PI_SESSION_FILE");
        if let Some(named) = named {
            command.env("PI_SESSION_FILE", named);
        }
        command.output().expect("the built stado starts")
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
fn a_pass_without_tama_runs_as_the_session_the_harness_names() {
    let run = Run::new();
    let transcript = run.transcript();

    let named = run.run(Some(&transcript));
    assert!(named.status.success(), "{}", said(&named));
    assert!(
        said(&named).contains(&format!("this pass runs inside agent session {SESSION}")),
        "the session PI_SESSION_FILE names was not read: {}",
        said(&named)
    );

    let resident = run.run(None);
    assert!(resident.status.success(), "{}", said(&resident));
    assert!(
        !said(&resident).contains("runs inside agent session"),
        "a pass no harness names became a session's: {}",
        said(&resident)
    );

    let gone = run.root.join("gone.jsonl");
    let missing = run.run(Some(&gone));
    assert!(
        said(&missing).contains(&format!(
            "PI_SESSION_FILE names {}, which cannot be opened",
            gone.display()
        )),
        "{}",
        said(&missing)
    );
    assert!(
        said(&missing).contains("unset PI_SESSION_FILE"),
        "the refusal says how to go on: {}",
        said(&missing)
    );

    let stranger = run.root.join("notes.jsonl");
    fs::write(&stranger, "{\"type\":\"title\"}\nnot json\n").unwrap();
    let headless = run.run(Some(&stranger));
    assert!(
        said(&headless).contains("hold no session record with an id"),
        "{}",
        said(&headless)
    );

    fs::remove_dir_all(&run.root).unwrap();
}
