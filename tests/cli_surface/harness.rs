//! The run the CLI surface journey records: the real `stado` binary of this
//! checkout in an isolated home, every command it ran with its exit status
//! and output kept in a report under `.build/`, and the two assertions the
//! journey makes of each object: its help lists its verbs, and a retired
//! name is refused by the parser.
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub(crate) struct Run {
    root: PathBuf,
    home: PathBuf,
    report: Value,
}

impl Run {
    pub(crate) fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let root = repository
            .join(".build/cli-surface-object-verbs")
            .join(uuid::Uuid::new_v4().to_string());
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        let source_revision = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repository)
            .output()
            .unwrap();
        let report = json!({
            "source_revision": String::from_utf8_lossy(&source_revision.stdout).trim(),
            "commands": [],
            "outcome": "failed",
        });
        Self { root, home, report }
    }

    pub(crate) fn stado(&mut self, args: &[&str]) -> std::process::Output {
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.home)
            .env("STADO_CONFIG", self.home.join(".stado/config.json"))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "args": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        self.save();
        output
    }

    fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }

    pub(crate) fn passed(mut self) {
        self.report["outcome"] = json!("passed");
        self.save();
        eprintln!("report: {}", self.root.join("report.json").display());
    }
}

/// The verbs each object's help must list, as the operator reads them.
pub(crate) fn help_lists(run: &mut Run, object: &[&str], verbs: &[&str]) {
    let mut args = object.to_vec();
    args.push("--help");
    let output = run.stado(&args);
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{} --help failed: {}",
        object.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    for verb in verbs {
        assert!(
            help.lines().any(|line| line.trim_start().starts_with(verb)),
            "{} --help does not list `{verb}`:\n{help}",
            object.join(" ")
        );
    }
}

/// A retired name is refused by the parser, naming the word it did not know.
pub(crate) fn refused_by_parser(run: &mut Run, args: &[&str], unknown: &str) {
    let output = run.stado(args);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "`stado {}` still parses: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        stderr.contains("unrecognized subcommand") && stderr.contains(unknown),
        "`stado {}` was not refused as an unknown subcommand naming `{unknown}`:\n{stderr}",
        args.join(" ")
    );
}
