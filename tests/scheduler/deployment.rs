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
        // run directory itself under a short name: one 16-bit field of a
        // random UUID in hex. A directory already holding that name is
        // refused rather than reused.
        let parent = repository.join(".build/z");
        fs::create_dir_all(&parent).unwrap();
        let root = parent.join(format!("{:x}", uuid::Uuid::new_v4().as_fields().1));
        if let Err(error) = fs::create_dir(&root) {
            panic!("{} cannot be this run's own: {error}", root.display());
        }
        fs::create_dir(root.join("tmp")).unwrap();
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
        let (succeeded, stdout, stderr) = self.run(args, "");
        assert!(succeeded, "stado {args:?}: {stderr}");
        stdout
    }

    /// Run one CLI command with `input` on its stdin, recording it; whether
    /// it succeeded, its stdout and its stderr.
    pub fn run(&mut self, args: &[&str], input: &str) -> (bool, String, String) {
        let mut child = self
            .command()
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "args": args,
            "exit_status": output.status.code(),
            "stdout": stdout,
            "stderr": stderr,
        }));
        self.save();
        (output.status.success(), stdout, stderr)
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
        self.report["service"] = json!({
            "arguments": args, "pid": child.id(),
            "stdout": "service.stdout", "stderr": "service.stderr",
        });
        let (sender, lines) = mpsc::channel();
        let streams: Vec<(Box<dyn std::io::Read + Send>, &str)> = vec![
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

    /// Every JSON document under `prefix/` of the local store; none while the
    /// prefix does not exist yet. A document the worker is still writing can
    /// be read part-written: it is left out of this read with its cause
    /// printed, and a later read finds it whole.
    pub fn documents(&self, prefix: &str) -> Vec<Value> {
        let directory = self.store().join(prefix);
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(error) => panic!("{} cannot be listed: {error}", directory.display()),
        };
        let mut documents = Vec::new();
        for entry in entries {
            let path = entry.unwrap().path();
            if !path.extension().is_some_and(|ext| ext == "json") {
                continue;
            }
            let read = fs::read(&path)
                .map_err(|error| error.to_string())
                .and_then(|bytes| {
                    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
                });
            match read {
                Ok(document) => documents.push(document),
                Err(cause) => eprintln!("{} left out of this read: {cause}", path.display()),
            }
        }
        documents
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

    fn stop_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            let status = match child.try_wait().expect("inspect isolated service exit") {
                Some(status) => status,
                None => {
                    child.kill().expect("stop isolated service");
                    child.wait().expect("retain isolated service exit")
                }
            };
            self.report["service"]["exit_status"] = json!(status.code());
            self.report["service"]["termination"] = json!(status.to_string());
        }
    }

    /// End the running `stado serve` as a crash ends it — its workloads, in
    /// their own process groups, are left as they are — and keep its logs
    /// beside the next run's under `before-restart.`.
    pub fn stop(&mut self) {
        self.stop_child();
        for log in self.logs.drain(..) {
            let _ = log.join();
        }
        let keep = |name: &str| {
            fs::rename(
                self.root.join(name),
                self.root.join(format!("before-restart.{name}")),
            )
            .unwrap();
        };
        keep("service.stdout");
        keep("service.stderr");
        self.report["service"]["stdout"] = json!("before-restart.service.stdout");
        self.report["service"]["stderr"] = json!("before-restart.service.stderr");
        self.report["previous_service"] = self.report["service"].take();
        self.save();
    }
}

impl Drop for Deployment {
    fn drop(&mut self) {
        self.stop_child();
        for log in self.logs.drain(..) {
            let _ = log.join();
        }
        self.save();
        eprintln!("{} evidence: {}", self.name, self.root.display());
    }
}

/// Read log lines until one carrying `needle` arrives, returning it.
pub fn line_with(lines: &Receiver<String>, needle: &str) -> String {
    line_with_any(lines, &[needle])
}

/// Read log lines until one carrying any of `needles` arrives, returning it.
pub fn line_with_any(lines: &Receiver<String>, needles: &[&str]) -> String {
    for line in lines.iter() {
        if needles.iter().any(|needle| line.contains(needle)) {
            return line;
        }
    }
    panic!("the service ended before logging any of {needles:?}; inspect service.stderr");
}
