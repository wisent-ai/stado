//! A measurement the queue records is used by the very next coordinator
//! tick: the sizing map is rebuilt when the completed and failed records
//! change, never after a window of its own.
//!
//! One isolated deployment: `config init` seeds a local store, a job naming
//! a model nothing has measured yet is submitted, and the real
//! `stado serve --control-plane local` ticks over it. Once a tick has passed,
//! a completed record measuring that model appears in the store; by the end
//! of the next tick that starts after it, the queued job must carry the
//! measured peak.
use serde_json::{json, Value};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;

const MODEL: &str = "sizing-journey/measured-model";
const MEASURED_PEAK_GB: i64 = 11;

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
        // A Unix socket path is at most 104 bytes on macOS, so HOME is the
        // run directory itself under a short name.
        let root = repository
            .join(".build/z")
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

    fn cli(&mut self, args: &[&str]) -> String {
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

    /// Run the local control plane and hand back its log lines.
    fn serve(&mut self) -> Receiver<String> {
        let args = [
            "serve",
            "--control-plane",
            "local",
            "--control-plane-interval-seconds",
            "5",
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
        self.child = Some(child);
        self.save();
        lines
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
        if let Some(mut child) = self.child.take() {
            child.kill().unwrap();
            child.wait().unwrap();
        }
        for log in self.logs.drain(..) {
            let _ = log.join();
        }
        self.save();
        eprintln!("sizing evidence: {}", self.root.display());
    }
}

/// Read log lines until one completed tick has been logged, returning it.
fn next_tick(lines: &Receiver<String>) -> String {
    for line in lines.iter() {
        if line.contains("tick scheduled=") || line.contains("tick failed:") {
            return line;
        }
    }
    panic!("the control plane ended before completing a tick; inspect service.stderr");
}

fn queued_rows(store: &std::path::Path) -> Vec<Value> {
    fs::read_dir(store.join("queue"))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
                .map(|path| serde_json::from_slice(&fs::read(path).unwrap()).unwrap())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn the_next_tick_sizes_a_queued_job_from_a_new_measurement() {
    let mut deployment = Deployment::start();
    let command = format!("true --model {MODEL}");
    deployment.cli(&["submit", "--run-id", "sizing-journey", &command]);
    let lines = deployment.serve();

    // A tick over a queue whose model nothing has measured.
    let first = next_tick(&lines);
    deployment.report["tick_before_measurement"] = json!(first);
    let job = queued_rows(&deployment.store())
        .into_iter()
        .next()
        .expect("the submitted job stays queued: nothing claims it here");
    assert_eq!(job["gpu_mem_gb"], json!(0), "unmeasured model: {job}");

    // The measurement a finished run of that model records.
    let completed = deployment.store().join("completed");
    fs::create_dir_all(&completed).unwrap();
    let record = json!({
        "job_id": "sizing-journey-measured",
        "state": "completed",
        "command": command,
        "peak_vram_gb": MEASURED_PEAK_GB,
        "peak_vram_per_gpu": true,
    });
    fs::write(
        completed.join("sizing-journey-measured.json"),
        serde_json::to_vec_pretty(&record).unwrap(),
    )
    .unwrap();
    deployment.report["measurement"] = record;

    // The tick in flight when the record landed may have listed before it;
    // the one after it cannot have.
    deployment.report["ticks_after_measurement"] = json!([next_tick(&lines), next_tick(&lines)]);
    let job = queued_rows(&deployment.store())
        .into_iter()
        .next()
        .expect("the job is still queued");
    deployment.report["queued_job"] = job.clone();
    assert_eq!(
        job["gpu_mem_gb"],
        json!(MEASURED_PEAK_GB),
        "the tick after the measurement must size the job from it: {job}"
    );
    deployment.report["outcome"] = json!("passed");
    deployment.save();
}
