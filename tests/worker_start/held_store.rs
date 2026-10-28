//! A worker whose fleet store does not answer at start holds and tries
//! again at its declared poll; it does not end the host's one Stado process.
//!
//! The real stado runs `serve --worker --standalone` with `HOME` under the
//! checkout's build directory and a fleet store that is a loopback listener
//! this test owns, which drops every connection after its request line, so
//! binding the store at start fails with the transport's cause. Expected:
//! the agent log says `the agent did not start: the fleet store did not
//! answer (…); the next poll tries again` at least twice, and the process is
//! still running when it has said so; the test then ends the process it
//! started. Ending the agent on that condition ended the whole service,
//! which launchd started again into the same unanswered store, a restart
//! loop that took the resolver and every adapter down for as long as the
//! store stood.

use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;

use serde_json::{json, Value};

const HELD: &str = "the agent did not start: the fleet store did not answer";
const TRIES_AGAIN: &str = "the next poll tries again";

struct Run {
    root: PathBuf,
    report: Value,
    worker: Option<Child>,
}

impl Run {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../.build/journeys/worker-start-held-store")
            .join(std::process::id().to_string());
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".stado")).expect("create the run directory");
        std::fs::create_dir_all(root.join("tmp")).expect("create the run's scratch directory");
        std::fs::create_dir_all(root.join("waits")).expect("create the wait register");
        let revision = Command::new("git")
            .arg("-C")
            .arg(env!("CARGO_MANIFEST_DIR"))
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git answers the checkout's revision");
        Self {
            report: json!({
                "source_revision": String::from_utf8_lossy(&revision.stdout).trim(),
                "worker_log": [],
                "outcome": "failed",
            }),
            root,
            worker: None,
        }
    }

    /// The real stado with nothing of this account's configuration.
    fn stado(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.root)
            .env(
                "STADO_CONFIG",
                concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/config/empty.json"),
            )
            .env("STADO_WAITS_DIR", self.root.join("waits"))
            .env("TMPDIR", self.root.join("tmp"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// A loopback address this machine hands out now, as the product binds one.
    fn free_bind(&self) -> String {
        let output = self
            .stado()
            .args(["host", "local", "port"])
            .output()
            .expect("run the real stado");
        assert!(
            output.status.success(),
            "stado host local port: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let printed = String::from_utf8_lossy(&output.stdout);
        match printed.trim().parse::<u16>() {
            Ok(port) => format!("{}:{port}", Ipv4Addr::LOCALHOST),
            Err(error) => panic!("stado host local port printed {printed:?}: {error}"),
        }
    }

    /// The fleet store: reads each connection's request line and drops it,
    /// so every request fails with the transport's cause.
    fn dropping_store(&self) -> String {
        let listener = TcpListener::bind(self.free_bind()).unwrap();
        let address = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut request_line = String::new();
                let _ = BufReader::new(&stream).read_line(&mut request_line);
            }
        });
        address
    }

    /// `stado serve --worker --standalone` against the store at `store`;
    /// its stderr lines arrive on the receiver.
    fn serve_worker(&mut self, store: &str) -> mpsc::Receiver<String> {
        let token = self.root.join("object-api.token");
        std::fs::write(&token, "worker-start-probe").unwrap();
        let mut child = self
            .stado()
            .args(["serve", "--worker", "--standalone", "--poll-seconds", "1"])
            .env("WC_STORAGE_BACKEND", "stado")
            .env("WC_STADO_STORAGE_URL", format!("http://{store}"))
            .env("WC_STADO_STORAGE_TOKEN_FILE", &token)
            .stdout(Stdio::null())
            .spawn()
            .expect("start the real worker");
        let stderr = child.stderr.take().unwrap();
        self.worker = Some(child);
        let (lines, received) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let (line, readable) = match line {
                    Ok(line) => (line, true),
                    Err(error) => (
                        format!("the worker's standard error could not be read: {error}"),
                        false,
                    ),
                };
                if lines.send(line).is_err() || !readable {
                    return;
                }
            }
        });
        received
    }

    fn log(&mut self, line: String) {
        self.report["worker_log"]
            .as_array_mut()
            .unwrap()
            .push(json!(line));
    }

    fn still_running(&mut self) -> bool {
        let worker = self.worker.as_mut().expect("the worker was started");
        worker
            .try_wait()
            .expect("the worker's state is readable")
            .is_none()
    }

    fn save(&self) {
        std::fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        if let Some(worker) = self.worker.as_mut() {
            let _ = worker.kill();
            let _ = worker.wait();
        }
        self.save();
        eprintln!(
            "retained report: {}",
            self.root.join("report.json").display()
        );
    }
}

#[test]
fn a_worker_holds_at_start_while_the_fleet_store_does_not_answer() {
    let mut run = Run::new();
    let store = run.dropping_store();
    let lines = run.serve_worker(&store);
    let mut held: Vec<String> = Vec::new();
    loop {
        let Ok(line) = lines.recv() else {
            run.save();
            panic!(
                "the worker stopped before holding on the store: {}",
                run.report["worker_log"]
            );
        };
        run.log(line.clone());
        if line.contains(HELD) {
            assert!(line.contains(TRIES_AGAIN), "{line}");
            assert!(line.contains(&store), "the hold names the store: {line}");
            held.push(line);
        }
        if matches!(held.as_slice(), [_, _, ..]) {
            break;
        }
    }
    assert!(
        run.still_running(),
        "the worker ended after holding on the store: {}",
        run.report["worker_log"]
    );
    run.report["outcome"] = json!("passed");
    run.save();
}
