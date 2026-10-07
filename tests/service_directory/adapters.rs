//! A changed resolver configuration rebinds the adapters that changed and
//! leaves the resolver, and every adapter that did not change, running.
//!
//! The resolver used to end on any configuration change, and as a role of
//! `stado serve` it took the whole host process, and every connection through
//! every forward, with it. One isolated deployment: a local store holding a
//! registry whose target declares adapter `kept`, and the real
//! `stado serve --resolver --target marker-host` reading it. The registry then
//! declares adapter `added` in place of `dropped`; the same resolver process
//! must say it started `added` and stopped `dropped`, still be running, still
//! accept on `kept`, accept on `added`, and no longer accept on `dropped`.
//! Changing the resolver's own API bind is the one change that still ends it,
//! and it must say so.
//!
//! The test follows the resolver's own log line by line, so every wait ends on
//! the resolver saying what it did, or on it exiting.
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Lines};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, ChildStderr, Command, Output, Stdio};

const TARGET: &str = "marker-host";
const SERVICE: &str = "marker-api";
const CONSUMER: &str = "marker-consumer";

/// A loopback address the kernel just handed out and nothing holds now.
fn free_bind() -> String {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.local_addr().unwrap().to_string()
}

fn adapter(bind: &str) -> Value {
    json!({"service": SERVICE, "consumer": CONSUMER, "bind": bind})
}

fn accepts(bind: &str) -> bool {
    TcpStream::connect(bind).is_ok()
}

struct Deployment {
    root: PathBuf,
    report: Value,
    resolver: Option<(Child, Lines<BufReader<ChildStderr>>)>,
}

impl Deployment {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let root = repository
            .join(".build/service-directory-adapters")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(root.join("tmp")).unwrap();
        fs::create_dir_all(root.join("store/ecosystem/probierz")).unwrap();
        Self {
            root,
            report: json!({
                "source_revision": std::env::var("STADO_SOURCE_REVISION").unwrap_or_default(),
                "commands": [],
                "resolver_log": [],
                "outcome": "failed",
            }),
            resolver: None,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap())
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .stdin(Stdio::null());
        command
    }

    fn run(&mut self, args: &[&str]) -> Output {
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

    fn set(&mut self, path: &str, value: &str) {
        self.cli(&["registry", "set", "--path", path, "--value", value]);
    }

    fn serve(&mut self) {
        let mut child = self
            .command()
            .args(["serve", "--resolver", "--target", TARGET])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let lines = BufReader::new(child.stderr.take().unwrap()).lines();
        self.resolver = Some((child, lines));
    }

    /// Read the resolver's log until a line contains `wanted`; the resolver
    /// exiting first ends the test with everything it said.
    fn await_line(&mut self, wanted: &str) -> String {
        loop {
            let line = self.resolver.as_mut().unwrap().1.next();
            let Some(Ok(line)) = line else {
                self.save();
                panic!(
                    "the resolver stopped before saying {wanted:?}: {}",
                    self.report["resolver_log"]
                );
            };
            self.report["resolver_log"]
                .as_array_mut()
                .unwrap()
                .push(json!(line));
            if line.contains(wanted) {
                self.save();
                return line;
            }
        }
    }

    fn running(&mut self) -> bool {
        self.resolver
            .as_mut()
            .unwrap()
            .0
            .try_wait()
            .unwrap()
            .is_none()
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
        if let Some((mut child, _)) = self.resolver.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn a_changed_directory_rebinds_only_the_adapters_that_changed() {
    let mut deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    let hostname = Command::new("hostname").output().unwrap();
    let hostname = String::from_utf8(hostname.stdout)
        .unwrap()
        .trim()
        .to_lowercase();
    let mut document: Value = serde_json::from_str(include_str!("registry.json")).unwrap();
    document["targets"][0]["hostnames"] = json!([hostname]);
    let (api, kept, dropped, added) = (free_bind(), free_bind(), free_bind(), free_bind());
    document["targets"][0]["service_resolver"]["api_bind"] = json!(api);
    document["targets"][0]["service_resolver"]["adapters"] =
        json!([adapter(&kept), adapter(&dropped)]);
    let registry = deployment.root.join("registry.json");
    fs::write(&registry, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let registry = registry.to_string_lossy().into_owned();
    deployment.cli(&["registry", "import", &registry]);

    deployment.serve();
    deployment.await_line(&format!("stado resolver target={TARGET}"));
    assert!(
        accepts(&kept) && accepts(&dropped),
        "both declared adapters accept"
    );

    let adapters = format!("targets.{TARGET}.service_resolver.adapters");
    deployment.set(
        &adapters,
        &json!([adapter(&kept), adapter(&added)]).to_string(),
    );
    deployment.await_line(&format!("stopped adapter service={SERVICE}"));
    deployment.await_line(&format!("started adapter service={SERVICE}"));
    assert!(
        deployment.running(),
        "the resolver keeps running through an adapter change"
    );
    assert!(
        accepts(&kept),
        "an adapter that did not change still accepts"
    );
    assert!(accepts(&added), "a newly declared adapter accepts");
    assert!(
        !accepts(&dropped),
        "an adapter no longer declared stops accepting"
    );

    // The one change it does not absorb, said in its own words.
    let next_api = free_bind();
    deployment.set(
        &format!("targets.{TARGET}.service_resolver.api_bind"),
        &next_api,
    );
    let said = deployment.await_line("resolver API bind changed");
    assert!(said.contains(&next_api), "{said}");

    deployment.report["outcome"] = json!("passed");
    deployment.save();
}
