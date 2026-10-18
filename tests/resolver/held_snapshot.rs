//! The registry authority's `stado resolver snapshot` answers from the
//! generation its running resolver holds, without reading the registry store.
//!
//! One isolated deployment, as in `tests/resolver/unserved.rs`: a local store
//! holding the shared directory template with this machine as `marker-host`,
//! which is also the directory's authority, and the real `stado serve
//! --resolver`. Expected: the snapshot prints the imported generation; with
//! the store moved away it still prints it, because nothing reads the store;
//! with the resolver stopped it is refused at once, naming the process that
//! is not running; and with a published resolver whose API accepts the
//! request and answers nothing it ends by itself at the published refresh
//! interval, naming that API. Read from the store itself, the snapshot on an
//! authority whose store stands would hold every host's registry read behind
//! it; asked of the resolver with no measure, every reader that gives up over
//! SSH would leave one snapshot process waiting on the authority.
use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};

use serde_json::{json, Value};

const TARGET: &str = "marker-host";
/// The line `stado serve --resolver` prints once it publishes `serving`.
const SERVING: &str = "stado resolver target=";

struct Deployment {
    root: PathBuf,
    resolver: Option<Child>,
}

impl Deployment {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the package sits inside the checkout")
            .to_path_buf();
        let root = repository
            .join(".build/resolver-held-snapshot")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        std::fs::create_dir_all(root.join("store")).unwrap();
        Self {
            root,
            resolver: None,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env(
                "PATH",
                std::env::var_os("PATH").expect("PATH names the tools"),
            )
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn run(&self, name: &str, args: &[&str]) -> Output {
        let output = self.command().args(args).output().unwrap();
        std::fs::write(self.root.join(format!("{name}.stdout")), &output.stdout).unwrap();
        std::fs::write(self.root.join(format!("{name}.stderr")), &output.stderr).unwrap();
        output
    }

    fn cli(&self, name: &str, args: &[&str]) -> String {
        let output = self.run(name, args);
        assert!(
            output.status.success(),
            "stado {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn free_bind(&self) -> String {
        let printed = self.cli("port", &["host", "local", "port"]);
        match printed.trim().parse() {
            Ok(port) => SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).to_string(),
            Err(error) => panic!("stado host local port printed {printed:?}: {error}"),
        }
    }

    /// The shared directory template with this machine as the reader and
    /// authority, its resolution API at `api_bind` and no adapter. Returns
    /// the directory generation it declares.
    fn import_registry(&self, api_bind: &str) -> u64 {
        let mut document: Value =
            serde_json::from_str(include_str!("../service_directory/registry.json")).unwrap();
        let target = document["targets"]
            .as_array_mut()
            .and_then(|targets| targets.first_mut())
            .unwrap();
        target["hostnames"] = json!([hostname()]);
        target["service_resolver"]["api_bind"] = json!(api_bind);
        target["service_resolver"]["adapters"] = json!([]);
        let generation = document["service_directory"]["generation"]
            .as_u64()
            .expect("the template declares a directory generation");
        let registry = self.root.join("registry.json");
        std::fs::write(&registry, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
        self.cli(
            "import",
            &["registry", "import", &registry.to_string_lossy()],
        );
        generation
    }

    fn serve(&mut self) {
        let mut child = self
            .command()
            .args(["serve", "--resolver", "--target", TARGET])
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().expect("the resolver's stderr is piped");
        let mut said = Vec::new();
        for line in BufReader::new(stderr).lines() {
            let line = line.expect("the resolver's stderr reads");
            said.push(line.clone());
            if line.starts_with(SERVING) {
                break;
            }
        }
        std::fs::write(self.root.join("resolver.stderr"), said.join("\n")).unwrap();
        assert!(
            said.iter().any(|line| line.starts_with(SERVING)),
            "the resolver ended without serving: {}",
            said.join("\n")
        );
        self.resolver = Some(child);
    }

    fn stop(&mut self) -> u32 {
        let mut child = self.resolver.take().expect("the resolver runs");
        let pid = child.id();
        child.kill().unwrap();
        child.wait().unwrap();
        pid
    }

    /// Move the store out of its declared place and back, so a read of it
    /// in between finds no registry.
    fn move_store(&self, from: &str, to: &str) {
        std::fs::rename(self.root.join(from), self.root.join(to)).unwrap();
    }

    /// The snapshot as every other host reads it: one JSON line with the
    /// store version and the document.
    fn snapshot_generation(&self, name: &str) -> u64 {
        let printed = self.cli(name, &["resolver", "snapshot"]);
        let payload: Value = match serde_json::from_str(printed.trim()) {
            Ok(payload) => payload,
            Err(error) => panic!("the snapshot is one JSON document ({error}): {printed}"),
        };
        assert!(
            payload["store_version"].is_string(),
            "the snapshot carries the store version: {printed}"
        );
        match payload["document"]["service_directory"]["generation"].as_u64() {
            Some(generation) => generation,
            None => panic!("the snapshot carries the directory generation: {printed}"),
        }
    }
}

impl Drop for Deployment {
    fn drop(&mut self) {
        if let Some(mut child) = self.resolver.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn hostname() -> String {
    let output = Command::new("hostname").output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .to_lowercase()
}

#[test]
fn the_authority_snapshot_answers_from_the_running_resolver_and_never_reads_the_store() {
    let mut deployment = Deployment::start();
    deployment.cli("config-init", &["config", "init"]);
    let api_bind = deployment.free_bind();
    let generation = deployment.import_registry(&api_bind);
    deployment.serve();

    assert_eq!(
        deployment.snapshot_generation("snapshot-served"),
        generation,
        "the snapshot prints the generation the resolver holds"
    );

    // No registry is where the store is declared now: a snapshot that still
    // answers came from the resolver's memory.
    deployment.move_store("store", "store-moved");
    let moved = deployment.run("snapshot-store-moved", &["resolver", "snapshot"]);
    deployment.move_store("store-moved", "store");
    assert!(
        moved.status.success(),
        "the snapshot answers while the store is gone: {}",
        String::from_utf8_lossy(&moved.stderr)
    );
    assert_eq!(
        deployment.snapshot_generation("snapshot-store-back"),
        generation,
        "the snapshot still prints the held generation"
    );

    let pid = deployment.stop();
    let refused = deployment.run("snapshot-stopped", &["resolver", "snapshot"]);
    let said = String::from_utf8_lossy(&refused.stderr).into_owned();
    assert!(
        !refused.status.success(),
        "a snapshot with no running resolver is refused: {said}"
    );
    assert!(
        said.contains(&format!(
            "the resolver that published its state (pid {pid}) is not running"
        )),
        "the refusal names the resolver that is not running: {said}"
    );

    // A published resolver that accepts the request and answers nothing: the
    // state names a live process (this test) and an API bind this test holds
    // and never answers on. The test holds no clock: the command must end by
    // itself, at the refresh interval the state publishes.
    let silent = std::net::TcpListener::bind(deployment.free_bind()).unwrap();
    let bind = silent.local_addr().unwrap().to_string();
    let state_path = deployment.root.join(".stado/resolver-state.json");
    let mut state: Value = serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
    assert!(
        state["refresh_seconds"].is_u64(),
        "the resolver publishes the interval its readers are measured by: {state}"
    );
    state["pid"] = json!(std::process::id());
    state["api_bind"] = json!(bind);
    std::fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
    let abandoned = deployment.run("snapshot-silent", &["resolver", "snapshot"]);
    drop(silent);
    let said = String::from_utf8_lossy(&abandoned.stderr).into_owned();
    assert!(
        !abandoned.status.success(),
        "a snapshot whose resolver answers nothing is refused: {said}"
    );
    assert!(
        said.contains(&format!(
            "the resolver answering at {bind} accepted the request and answered nothing"
        )),
        "the refusal names the resolver API that answered nothing: {said}"
    );
}
