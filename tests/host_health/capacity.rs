//! A worker's capacity publication states when its publisher will publish
//! again: readers judge the row by that promise, never by a window of their
//! own.
//!
//! One isolated deployment: `config init` seeds a local registry naming this
//! machine and a local store. The real `stado serve --api --worker
//! --poll-seconds 1` runs; once its log says it published, the stored row must
//! carry `next_by` at least one poll period after `published_at`, and no
//! window of its own (`stale_after_seconds`).
use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread::JoinHandle;

struct Deployment {
    root: PathBuf,
    report: Value,
    child: Option<Child>,
    logs: Vec<JoinHandle<()>>,
}

impl Deployment {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        // The process binds `$HOME/.stado/release-proxy.sock`, and a Unix
        // socket path is at most 104 bytes on macOS, so HOME is the run
        // directory itself under a short name.
        let root = repository
            .join(".build/c")
            .join(&uuid::Uuid::new_v4().simple().to_string()[..6]);
        fs::create_dir_all(root.join("tmp")).unwrap();
        let mut deployment = Self {
            root,
            report: json!({
                "source_revision": std::env::var("STADO_SOURCE_REVISION").unwrap_or_default(),
                "commands": [],
                "outcome": "failed",
            }),
            child: None,
            logs: Vec::new(),
        };
        deployment.cli(&["config", "init"]);
        // The fleet's deployments name their queue namespace, as the
        // beacon journey does; the registry is read through it.
        deployment.cli(&["config", "set", "storage.stado.namespace", "probierz"]);
        deployment
    }

    fn store(&self) -> PathBuf {
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

    fn run(&mut self, args: &[&str]) -> std::process::Output {
        let output = self.command().args(args).output().unwrap();
        self.report["commands"].as_array_mut().unwrap().push(json!({
            "args": args,
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
        self.save();
        output
    }

    fn cli(&mut self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "stado {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Run the host process with the worker role on a one-second poll and
    /// return once its log says the first capacity publication was accepted.
    fn serve(&mut self) {
        let limits = std::env::var("STADO_TEST_REQUEST_LIMITS")
            .expect("STADO_TEST_REQUEST_LIMITS must declare the qualification API byte bounds");
        self.cli(&["config", "set", "dashboard.request_limits", &limits]);
        let store = self.store().to_string_lossy().into_owned();
        let bind = std::net::Ipv4Addr::LOCALHOST.to_string();
        let args = [
            "serve",
            "--api",
            "--bind",
            &bind,
            "--port",
            "0",
            "--api-local-store",
            &store,
            "--worker",
            "--kind",
            "local",
            "--poll-seconds",
            "1",
        ];
        let mut child = self
            .command()
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
        drop(sender);
        self.child = Some(child);
        self.save();
        for line in lines.iter() {
            if line.contains("[agent] loop:") && line.contains(": published accepting_jobs=") {
                self.report["first_publication"] = json!(line);
                self.save();
                return;
            }
        }
        panic!("the worker ended before publishing capacity; inspect service.stderr");
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            child.kill().unwrap();
            child.wait().unwrap();
        }
    }

    fn pass(&mut self) {
        self.report["outcome"] = json!("passed");
        self.save();
    }

    fn save(&self) {
        fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }
}

impl Drop for Deployment {
    fn drop(&mut self) {
        self.stop();
        for log in self.logs.drain(..) {
            let _ = log.join();
        }
        self.save();
        eprintln!("capacity evidence: {}", self.root.display());
    }
}

fn capacity_rows(store: &std::path::Path) -> Vec<(PathBuf, Value)> {
    // The agent of a deployment whose store is local writes its queue
    // objects at the store's root.
    let directory = store.join("capacity");
    fs::read_dir(&directory)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
                .map(|path| {
                    let row = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                    (path, row)
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_worker_capacity_row_states_its_next_publication() {
    let mut deployment = Deployment::start();
    deployment.serve();
    let rows = capacity_rows(&deployment.store());
    assert_eq!(rows.len(), 1, "one worker publishes one row: {rows:?}");
    let (_, row) = rows.into_iter().next().unwrap();
    let stamp = |field: &str| {
        chrono::DateTime::parse_from_rfc3339(row[field].as_str().unwrap_or_default())
            .unwrap_or_else(|error| panic!("{field} must be RFC 3339 ({error}): {row}"))
    };
    assert!(
        stamp("next_by") - stamp("published_at") >= chrono::Duration::seconds(1),
        "next_by must be at least one poll after published_at: {row}"
    );
    assert!(
        row.get("stale_after_seconds").is_none(),
        "the publisher states its own next publication, not a window: {row}"
    );
    deployment.report["published_row"] = row;
    deployment.pass();
}
