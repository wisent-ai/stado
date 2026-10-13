//! The isolated run `main.rs` drives: the real stado with `HOME` under the
//! checkout's build directory, a fleet store that stops answering on demand,
//! a service that answers, and a report of every command kept beside it,
//! bound to the checkout's revision.
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStderr, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use serde_json::{json, Value};

pub(crate) const TARGET: &str = "marker-host";
pub(crate) const SERVICE: &str = "marker-api";
const CONSUMER: &str = "marker-consumer";
const AUTHORITY: &str = "other-host";
const AUTHORITY_SSH: &str = "other-host.invalid";
const AUTHORITY_STADO: &str = "/usr/local/bin/stado";
pub(crate) const ANSWER: &str = "answered by marker-api";

/// The fleet store: resets every connection until `silent` is set, then
/// hands every connection it accepts to `held` and answers none.
pub(crate) struct Store {
    pub(crate) address: String,
    pub(crate) silent: Arc<AtomicBool>,
    pub(crate) held: mpsc::Receiver<TcpStream>,
}

pub(crate) struct Run {
    root: PathBuf,
    pub(crate) report: Value,
    resolver: Option<Child>,
}

impl Run {
    pub(crate) fn new() -> Self {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join("host-silence-refusal-in-flight")
            .join(std::process::id().to_string());
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".stado/cache")).expect("create the run directory");
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
                "commands": [],
                "resolver_log": [],
                "outcome": "failed",
            }),
            root,
            resolver: None,
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
            .env("STADO_CONFIG", self.root.join("no-config.toml"))
            .env("STADO_WAITS_DIR", self.root.join("waits"))
            .env("TMPDIR", self.root.join("tmp"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    pub(crate) fn cli(&mut self, args: &[&str]) -> String {
        let output = self
            .stado()
            .args(args)
            .output()
            .expect("run the real stado");
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

    /// A loopback address this machine hands out now, as the product binds one.
    pub(crate) fn free_bind(&mut self) -> String {
        let printed = self.cli(&["host", "local", "port"]);
        match printed.trim().parse::<u16>() {
            Ok(port) => format!("{}:{port}", Ipv4Addr::LOCALHOST),
            Err(error) => panic!("stado host local port printed {printed:?}: {error}"),
        }
    }

    pub(crate) fn store(&mut self) -> Store {
        let listener = TcpListener::bind(self.free_bind()).unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let silent = Arc::new(AtomicBool::default());
        let switch = Arc::clone(&silent);
        let (keep, held) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if switch.load(Ordering::SeqCst) {
                    if keep.send(stream).is_err() {
                        return;
                    }
                } else {
                    let mut request_line = String::new();
                    let _ = BufReader::new(&stream).read_line(&mut request_line);
                }
            }
        });
        Store {
            address,
            silent,
            held,
        }
    }

    /// The service the adapter relays to: answers every request and closes.
    pub(crate) fn answering_service(&mut self) -> String {
        let listener = TcpListener::bind(self.free_bind()).unwrap();
        let address = listener.local_addr().unwrap().to_string();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut request_line = String::new();
                let _ = BufReader::new(&stream).read_line(&mut request_line);
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{ANSWER}",
                    ANSWER.len()
                );
            }
        });
        address
    }

    /// The copy the resolver starts from: this machine as `marker-host`
    /// with one adapter for `marker-api`, stale after the template's refresh
    /// interval, and an authority nobody reaches.
    pub(crate) fn write_last_good(&self, api_bind: &str, adapter_bind: &str, service: &str) {
        let mut document: Value =
            serde_json::from_str(include_str!("../../service_directory/registry.json")).unwrap();
        for target in document["targets"].as_array_mut().unwrap().iter_mut() {
            match target["name"].as_str() {
                Some(TARGET) => {
                    let resolver = &mut target["service_resolver"];
                    resolver["max_stale_seconds"] = resolver["refresh_seconds"].clone();
                    resolver["api_bind"] = json!(api_bind);
                    resolver["adapters"] =
                        json!([{"service": SERVICE, "consumer": CONSUMER, "bind": adapter_bind}]);
                    target["hostnames"] = json!([hostname()]);
                }
                Some(AUTHORITY) => target["ssh"] = json!(AUTHORITY_SSH),
                _ => {}
            }
        }
        document["service_directory"]["authority"] =
            json!({"target": AUTHORITY, "command": AUTHORITY_STADO});
        document["service_directory"]["services"][SERVICE]["endpoints"][TARGET]["url"] =
            json!(format!("http://{service}"));
        std::fs::write(
            self.root.join(".stado/cache/registry-last-good.json"),
            serde_json::to_vec_pretty(&document).unwrap(),
        )
        .unwrap();
        std::fs::write(
            self.root.join(".stado/cache/registry-last-good.meta.json"),
            serde_json::to_vec(&json!({
                "read_at": "1970-01-01T00:00:00Z",
                "generation": "example",
            }))
            .unwrap(),
        )
        .unwrap();
    }

    /// `stado serve --resolver` with the fleet store at `store`, followed
    /// until it says it serves; the rest of its log arrives on the receiver.
    pub(crate) fn serve(&mut self, store: &str) -> mpsc::Receiver<String> {
        let token = self.root.join("object-api.token");
        std::fs::write(&token, "refusal-in-flight-probe").unwrap();
        let mut child = self
            .stado()
            .args(["serve", "--resolver", "--target", TARGET])
            .env("WC_STORAGE_BACKEND", "stado")
            .env("WC_STADO_STORAGE_URL", format!("http://{store}"))
            .env("WC_STADO_STORAGE_TOKEN_FILE", &token)
            .stdout(Stdio::null())
            .spawn()
            .expect("start the real resolver");
        let stderr: ChildStderr = child.stderr.take().unwrap();
        self.resolver = Some(child);
        let (lines, received) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if lines.send(line).is_err() {
                    return;
                }
            }
        });
        let serving = format!("stado resolver target={TARGET}");
        loop {
            let Ok(line) = received.recv() else {
                self.save();
                panic!(
                    "the resolver stopped before serving: {}",
                    self.report["resolver_log"]
                );
            };
            self.log(line.clone());
            if line.contains(&serving) {
                return received;
            }
        }
    }

    pub(crate) fn log(&mut self, line: String) {
        self.report["resolver_log"]
            .as_array_mut()
            .unwrap()
            .push(json!(line));
    }

    /// The resolver's open waits on `store`, as its wait register holds them.
    /// A file gone between listing and reading is a wait that ended.
    pub(crate) fn store_waits(&self, store: &str) -> Vec<String> {
        let place = format!("gdzie: object API http://{store}");
        let mut waits = Vec::new();
        for entry in std::fs::read_dir(self.root.join("waits")).unwrap() {
            let path = entry.unwrap().path();
            let line = match std::fs::read_to_string(&path) {
                Ok(line) => line,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => panic!(
                    "the wait register entry {} is unreadable: {error}",
                    path.display()
                ),
            };
            if line.contains(&place) {
                waits.push(line.trim().to_string());
            }
        }
        waits.sort();
        waits
    }

    pub(crate) fn save(&self) {
        std::fs::write(
            self.root.join("report.json"),
            serde_json::to_vec_pretty(&self.report).unwrap(),
        )
        .unwrap();
    }
}

impl Drop for Run {
    fn drop(&mut self) {
        if let Some(mut child) = self.resolver.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn hostname() -> String {
    let output = Command::new("hostname").output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .to_lowercase()
}
