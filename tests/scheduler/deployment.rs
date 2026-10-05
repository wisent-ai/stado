//! One isolated Stado deployment for a scheduler journey: its own HOME,
//! config and local store under the repository's ignored `.build/`, the real
//! `stado` binary, and a report of every command, exit status and output.

use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;

pub struct Deployment {
    pub root: PathBuf,
    pub report: Value,
    name: &'static str,
    child: Option<Child>,
    logs: Vec<JoinHandle<()>>,
}

impl Deployment {
    /// `config init` in a fresh run directory named after the journey.
    pub fn start(name: &'static str) -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        // A Unix socket path is at most 104 bytes on macOS, so HOME is the
        // run directory itself under a short name.
        let root = repository
            .join(".build/z")
            .join(&uuid::Uuid::new_v4().simple().to_string()[..6]);
        fs::create_dir_all(root.join("tmp")).unwrap();
        let mut deployment = Self {
            root,
            report: json!({
                "journey": name,
                "source_revision": std::env::var("STADO_SOURCE_REVISION").unwrap_or_default(),
                "commands": [],
                "outcome": "failed",
            }),
            name,
            child: None,
            logs: Vec::new(),
        };
        deployment.cli(&["config", "init"]);
        deployment
    }

    pub fn store(&self) -> PathBuf {
        self.root.join(".stado").join("local-storage")
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .stdin(Stdio::null());
        command
    }

    /// Run one CLI command and require it to succeed; its stdout.
    pub fn cli(&mut self, args: &[&str]) -> String {
        let output = self.command().args(args).output().unwrap();
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "args": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        self.save();
        assert!(
            output.status.success(),
            "stado {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Run `stado serve` with `args` and hand back its log lines as they come.
    pub fn serve(&mut self, args: &[&str]) -> Receiver<String> {
        let mut child = self
            .command()
            .arg("serve")
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        self.report["service"] = json!({"arguments": args, "pid": child.id()});
        let (sender, lines) = mpsc::channel();
        let streams: [(Box<dyn std::io::Read + Send>, &str); 2] = [
            (Box::new(child.stdout.take().unwrap()), "service.stdout"),
            (Box::new(child.stderr.take().unwrap()), "service.stderr"),
        ];
        for (stream, name) in streams {
            let path = self.root.join(name);
            let sender = sender.clone();
            self.logs.push(std::thread::spawn(move || {
                let mut file = File::create(path).unwrap();
                for line in BufReader::new(stream).lines() {
                    let line = line.unwrap();
                    writeln!(file, "{line}").unwrap();
                    file.flush().unwrap();
                    let _ = sender.send(line);
                }
            }));
        }
        self.child = Some(child);
        self.save();
        lines
    }

    /// Every JSON document under `prefix/` of the local store.
    pub fn documents(&self, prefix: &str) -> Vec<Value> {
        fs::read_dir(self.store().join(prefix))
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
                    .filter_map(|path| serde_json::from_slice(&fs::read(path).ok()?).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn pass(&mut self) {
        self.report["outcome"] = json!("passed");
        self.save();
    }

    pub fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }
}

impl Drop for Deployment {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            child.kill().unwrap();
            child.wait().unwrap();
        }
        for log in self.logs.drain(..) {
            let _ = log.join();
        }
        self.save();
        eprintln!("{} evidence: {}", self.name, self.root.display());
    }
}

/// Read log lines until one carrying `needle` arrives, returning it.
#[allow(dead_code)]
pub fn line_with(lines: &Receiver<String>, needle: &str) -> String {
    line_with_any(lines, &[needle])
}

/// Read log lines until one carrying any of `needles` arrives, returning it.
#[allow(dead_code)]
pub fn line_with_any(lines: &Receiver<String>, needles: &[&str]) -> String {
    for line in lines.iter() {
        if needles.iter().any(|needle| line.contains(needle)) {
            return line;
        }
    }
    panic!("the service ended before logging any of {needles:?}; inspect service.stderr");
}
