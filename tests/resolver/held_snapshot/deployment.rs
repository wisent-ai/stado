//! The journey's isolated deployment: a home and local store of its own
//! under the checkout's build directory, the real `stado` run against them,
//! and the resolver it serves.

use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};

use serde_json::{json, Value};

const TARGET: &str = "marker-host";
/// The line `stado serve --resolver` prints once it publishes `serving`.
const SERVING: &str = "stado resolver target=";

pub(crate) struct Deployment {
    pub(crate) root: PathBuf,
    resolver: Option<Child>,
}

impl Deployment {
    pub(crate) fn start() -> Self {
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

    pub(crate) fn run(&self, name: &str, args: &[&str]) -> Output {
        let output = self.command().args(args).output().unwrap();
        std::fs::write(self.root.join(format!("{name}.stdout")), &output.stdout).unwrap();
        std::fs::write(self.root.join(format!("{name}.stderr")), &output.stderr).unwrap();
        output
    }

    pub(crate) fn cli(&self, name: &str, args: &[&str]) -> String {
        let output = self.run(name, args);
        assert!(
            output.status.success(),
            "stado {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    pub(crate) fn free_bind(&self) -> String {
        let printed = self.cli("port", &["host", "local", "port"]);
        match printed.trim().parse() {
            Ok(port) => SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).to_string(),
            Err(error) => panic!("stado host local port printed {printed:?}: {error}"),
        }
    }

    /// The shared directory template with this machine as the reader and
    /// authority, its resolution API at `api_bind` and no adapter. Returns
    /// the directory generation it declares.
    pub(crate) fn import_registry(&self, api_bind: &str) -> u64 {
        let mut document: Value =
            serde_json::from_str(include_str!("../../service_directory/registry.json")).unwrap();
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

    pub(crate) fn serve(&mut self) {
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

    pub(crate) fn stop(&mut self) -> u32 {
        let mut child = self.resolver.take().expect("the resolver runs");
        let pid = child.id();
        child.kill().unwrap();
        child.wait().unwrap();
        pid
    }

    /// Move the store out of its declared place and back, so a read of it
    /// in between finds no registry.
    pub(crate) fn move_store(&self, from: &str, to: &str) {
        std::fs::rename(self.root.join(from), self.root.join(to)).unwrap();
    }

    /// The snapshot as every other host reads it: one JSON line with the
    /// store version and the document.
    pub(crate) fn snapshot_generation(&self, name: &str) -> u64 {
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
