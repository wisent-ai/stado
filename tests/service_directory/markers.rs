//! A host's forward markers follow its service directory without anyone
//! running `stado service directory publish` on it.
//!
//! One isolated deployment: a local store holding `registry.json`, which places
//! `marker-api` on `marker-host` and `elsewhere-api` only on `other-host`, and
//! the real `stado serve --resolver --target marker-host` reading it. Once the
//! resolver announces itself, `~/.stado/forwards/marker-api.local` must hold
//! the declared address, `elsewhere-api` must have no marker, and a marker the
//! directory does not declare must still be there. After the endpoint moves
//! and the directory generation advances, the resolver's next refresh must
//! rewrite the marker with the new address.
//!
//! The test follows the resolver's own log line by line, so every wait ends
//! on the resolver saying what it did, or on it exiting.
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Lines};
use std::path::PathBuf;
use std::process::{Child, ChildStderr, Command, Output, Stdio};

const TARGET: &str = "marker-host";
const SERVED: &str = "marker-api";
const ELSEWHERE: &str = "elsewhere-api";

fn declared() -> Value {
    serde_json::from_str(include_str!("registry.json")).unwrap()
}

fn endpoint(service: &str, host: &str) -> String {
    declared()["service_directory"]["services"][service]["endpoints"][host]["url"]
        .as_str()
        .unwrap()
        .to_string()
}

/// The same origin one port further on: where the test moves the endpoint.
fn moved(url: &str) -> String {
    let mut parsed = url::Url::parse(url).unwrap();
    let port = parsed.port().unwrap();
    parsed.set_port(Some(port + 1)).unwrap();
    parsed.as_str().trim_end_matches('/').to_string()
}

struct Resolver {
    child: Child,
    lines: Lines<BufReader<ChildStderr>>,
}

struct Deployment {
    root: PathBuf,
    report: Value,
    resolver: Option<Resolver>,
}

impl Deployment {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let root = repository
            .join(".build/service-directory-markers")
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

    fn serve(&mut self) {
        let mut child = self
            .command()
            .args(["serve", "--resolver", "--target", TARGET])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let lines = BufReader::new(child.stderr.take().unwrap()).lines();
        self.resolver = Some(Resolver { child, lines });
    }

    /// Read the resolver's log until a line contains `wanted`. The resolver
    /// exiting first ends the log, and the test, with everything it said.
    fn await_line(&mut self, wanted: &str) {
        loop {
            let line = self.resolver.as_mut().unwrap().lines.next();
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
                return;
            }
        }
    }

    fn marker(&self, service: &str) -> Option<String> {
        fs::read_to_string(
            self.root
                .join(".stado/forwards")
                .join(format!("{service}.local")),
        )
        .ok()
        .map(|text| text.trim().to_string())
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
        if let Some(mut resolver) = self.resolver.take() {
            let _ = resolver.child.kill();
            let _ = resolver.child.wait();
        }
    }
}

#[test]
fn the_resolver_keeps_the_markers_its_directory_declares() {
    let mut deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    // The resolver serves the target that names the machine it runs on, so
    // the isolated registry names this machine as `marker-host`.
    let hostname = Command::new("hostname").output().unwrap();
    let hostname = String::from_utf8(hostname.stdout)
        .unwrap()
        .trim()
        .to_lowercase();
    let mut document = declared();
    document["targets"][0]["hostnames"] = json!([hostname]);
    // `resolver status` probes the API bind, so it has to be a port the
    // resolver can actually hold: one the kernel just handed out.
    let free = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    document["targets"][0]["service_resolver"]["api_bind"] =
        json!(free.local_addr().unwrap().to_string());
    drop(free);
    let registry = deployment.root.join("registry.json");
    fs::write(&registry, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
    let registry = registry.to_string_lossy().into_owned();
    deployment.cli(&["registry", "import", &registry]);

    // A marker some earlier publish left for a service the directory no
    // longer declares: the keeper leaves it for `publish --prune`.
    let served = endpoint(SERVED, TARGET);
    let retired = endpoint(ELSEWHERE, "other-host");
    let forwards = deployment.root.join(".stado/forwards");
    fs::create_dir_all(&forwards).unwrap();
    fs::write(forwards.join("retired-api.local"), format!("{retired}\n")).unwrap();

    deployment.serve();
    // Announced after the markers are kept and the state is published.
    deployment.await_line(&format!("stado resolver target={TARGET}"));
    let status = deployment.cli(&["resolver", "status", "--target", TARGET, "--json"]);
    let status: Value = serde_json::from_str(&status).unwrap();
    assert_eq!(status["state"], "serving", "{status}");

    assert_eq!(
        deployment.marker(SERVED).as_deref(),
        Some(served.as_str()),
        "the directory's endpoint for this host is written before the resolver reports serving"
    );
    assert_eq!(
        deployment.marker(ELSEWHERE),
        None,
        "a service the directory gives this host no address for gets no marker"
    );
    assert_eq!(
        deployment.marker("retired-api").as_deref(),
        Some(retired.as_str()),
        "an undeclared marker is left for `directory publish --prune`, never removed here"
    );

    // Move the endpoint, then advance the generation: the resolver accepts a
    // changed directory only under a new generation.
    let next = moved(&served);
    deployment.cli(&[
        "registry",
        "set",
        "--path",
        &format!("service_directory.services.{SERVED}.endpoints.{TARGET}.url"),
        "--value",
        &next,
    ]);
    let generation = declared()["service_directory"]["generation"]
        .as_u64()
        .unwrap()
        + 1;
    deployment.cli(&[
        "registry",
        "set",
        "--path",
        "service_directory.generation",
        "--value",
        &generation.to_string(),
    ]);
    // Said once the refresh that loaded it has kept the markers.
    deployment.await_line(&format!(
        "stado resolver loaded directory generation {generation}"
    ));
    assert_eq!(
        deployment.marker(SERVED).as_deref(),
        Some(next.as_str()),
        "a refresh that loads a moved endpoint rewrites the marker"
    );

    deployment.report["outcome"] = json!("passed");
    deployment.save();
}
