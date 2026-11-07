//! The left-out journey's isolated world: a bare origin and a checkout under
//! the repository's ignored `.build/`, a home whose `~/.stado/bin/oko` is the
//! Oko this machine has installed, and that Oko's rulings and defect
//! registers of the run's own, so the operator's are never read or written.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const OWNER: &str = "change-left-out-test";
const NAME: &str = "product";
/// A product of the compiled catalog with a `submit` declaration.
const PRODUCT: &str = "trends";
pub const DEFECT: &str = "00000000-0000-4000-8000-0000000c1eff";
pub const PLATFORM: &str = "darwin-arm64";
pub const GATE: &str = "unit";
/// The ruling that withdraws tests in this run's register, and its words:
/// Oko quotes both when it refuses, so the record shows this ruling decided.
pub const RULING: &str = "0000C1EF-0000-4000-8000-00000000C1EF";
pub const SAID: &str = "nie testowac";

pub struct Journey {
    pub root: PathBuf,
    pub checkout: PathBuf,
    pub report: Value,
}

impl Journey {
    pub fn start() -> Self {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let root = repository
            .join(".build/change-left-out")
            .join(uuid::Uuid::new_v4().simple().to_string());
        let checkout = root.join("checkout");
        let bin = root.join("home/.stado/bin");
        for directory in [&checkout, &bin, &root.join("workspace"), &root.join("tmp")] {
            fs::create_dir_all(directory).unwrap();
        }
        // The Oko this machine runs, reached where Stado looks for it under
        // the run's own home.
        let operator =
            PathBuf::from(std::env::var_os("HOME").expect("HOME names the operator's home"));
        let oko = operator.join(".stado/bin/oko");
        assert!(
            oko.is_file(),
            "{} is not installed: this journey asks the real Oko",
            oko.display()
        );
        std::os::unix::fs::symlink(&oko, bin.join("oko")).unwrap();
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let ruling = json!({
            "id": RULING, "subjects": ["no-test"],
            "session": "left-out-session", "ordinal": 1, "said": SAID,
            "saidAt": now, "recordedIn": "left-out-recorder", "recordedAt": now,
        });
        fs::write(
            root.join("rulings.json"),
            json!({ "rulings": [ruling] }).to_string(),
        )
        .unwrap();
        let defect = json!({
            "id": DEFECT, "product": PRODUCT, "title": "a defect the journey's candidate repairs",
            "surface": "stado change run", "evidence": "written by this journey",
            "impact": "none outside it", "found_in": "left-out-session", "found_at": now,
        });
        fs::write(
            root.join("defects.json"),
            json!({ "defects": [defect] }).to_string(),
        )
        .unwrap();
        // The factory stands at install, so the rulings alone decide (Oko's
        // work stage holds a build back otherwise).
        fs::write(root.join("stage.json"), r#"{"stage":"install"}"#).unwrap();
        let journey = Self {
            root,
            checkout,
            report: json!({
                "journey": "change-left-out",
                "source_revision": env!("STADO_SOURCE_REVISION"),
                "commands": [],
                "outcome": "failed",
            }),
        };
        journey.save();
        journey
    }

    pub fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }

    fn keep(&mut self, program: &str, args: &[&str], output: &std::process::Output) {
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "program": program,
            "args": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        self.save();
    }

    fn git(&mut self, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(&self.checkout)
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", self.root.join("home"))
            .args(args)
            .output()
            .unwrap();
        self.keep("git", args, &output);
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// `stado args`: whether it succeeded, its standard output, and its
    /// standard output then error output for messages.
    pub fn stado(&mut self, args: &[&str]) -> (bool, String, String) {
        self.stado_as(None, args)
    }

    /// [`Journey::stado`] run as agent session `session` names it to Stado:
    /// `PI_SESSION_FILE` naming a transcript that opens with its record.
    pub fn stado_as(&mut self, session: Option<&str>, args: &[&str]) -> (bool, String, String) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .current_dir(&self.checkout)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", self.root.join("home"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "STADO_CONFIG",
                concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/config/empty.json"),
            )
            .env("STADO_CHANGE_QUEUE", self.root.join("queue"))
            .env("WISENT_WORKSPACE", self.root.join("workspace"))
            .env("OKO_RULING_REGISTER", self.root.join("rulings.json"))
            .env("OKO_DEFECT_REGISTER", self.root.join("defects.json"))
            .env("OKO_WORK_STAGE", self.root.join("stage.json"))
            .env("OKO_CLAIM_REGISTER", self.root.join("claims.json"))
            .env("OKO_PURGATORY_REGISTER", self.root.join("purgatory.json"))
            .env(
                "OKO_JUDGE_PROVIDER_PATH",
                self.root.join("judge-provider.json"),
            )
            .stdin(Stdio::null())
            .args(args);
        if let Some(session) = session {
            let transcript = self.root.join(format!("{session}.jsonl"));
            let record = json!({"type": "session", "version": 3, "id": session});
            fs::write(&transcript, format!("{record}\n")).unwrap();
            command.env("PI_SESSION_FILE", transcript);
        }
        let output = command.output().unwrap();
        self.keep("stado", args, &output);
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let said = format!("{stdout}{}", String::from_utf8_lossy(&output.stderr));
        (output.status.success(), stdout, said)
    }

    pub fn commit(&mut self, file: &str, text: &str, message: &str) -> String {
        fs::write(self.checkout.join(file), text).unwrap();
        self.git(&["add", file]);
        self.git(&["commit", "--quiet", "-m", message]);
        self.git(&["rev-parse", "HEAD"])
    }

    /// A bare `origin` holding `main`, whose release manifest declares one
    /// test gate for one platform, and a checkout of it.
    pub fn repository(&mut self) {
        let remote = self
            .root
            .join("remote")
            .join(OWNER)
            .join(format!("{NAME}.git"));
        fs::create_dir_all(&remote).unwrap();
        let remote = remote.to_string_lossy().into_owned();
        self.git(&[
            "init",
            "--quiet",
            "--bare",
            "--initial-branch=main",
            &remote,
        ]);
        self.git(&["init", "--quiet", "--initial-branch=main"]);
        self.git(&["config", "user.name", "Left Out Journey"]);
        self.git(&["config", "user.email", "left-out-journey@example.invalid"]);
        self.git(&["remote", "add", "origin", &remote]);
        let own: Value = serde_json::from_slice(
            &fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.wisent-release.json"))
                .unwrap(),
        )
        .unwrap();
        let manifest = json!({
            "schema_version": own["schema_version"],
            "product": PRODUCT,
            "releases": true,
            "version_source": {"kind": "text", "path": "VERSION"},
            "platforms": {
                PLATFORM: {
                    "runner_platform": PLATFORM,
                    "quality": [{"name": GATE, "kind": "test", "argv": ["true"]}],
                    "build": {"argv": ["cat", "VERSION"]},
                    "stage": {"VERSION": "VERSION"}
                }
            },
            "promotion": {"channels": ["candidate"], "reconcile": false}
        });
        fs::write(self.checkout.join("VERSION"), "0.1.0\n").unwrap();
        self.git(&["add", "VERSION"]);
        self.commit(
            ".wisent-release.json",
            &serde_json::to_string_pretty(&manifest).unwrap(),
            "declare the gate",
        );
        self.git(&["push", "--quiet", "origin", "HEAD:refs/heads/main"]);
    }

    /// The commit the bare origin's `main` names now.
    pub fn origin_main(&mut self) -> String {
        let listed = self.git(&["ls-remote", "origin", "refs/heads/main"]);
        listed
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string()
    }

    pub fn candidate(&self, id: &str) -> Value {
        let path = self
            .root
            .join("queue")
            .join(OWNER)
            .join(NAME)
            .join(format!("{id}.json"));
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap()
    }
}
