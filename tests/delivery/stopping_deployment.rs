//! One isolated deployment whose service stops on every start: a local
//! store holding the shared directory template with this machine as
//! `marker-host`, the resolution API's bind held by the test, and the real
//! `oko` keeping its defects in the deployment's own register.
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

pub(crate) const TARGET: &str = "marker-host";

pub(crate) struct Deployment {
    root: PathBuf,
    path: std::ffi::OsString,
}

impl Deployment {
    pub(crate) fn start(path: std::ffi::OsString) -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the package sits inside the checkout")
            .to_path_buf();
        let root = repository
            .join(".build/delivery-repeated-stops")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        std::fs::create_dir_all(root.join("store")).unwrap();
        Self { root, path }
    }

    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
        command
            .env_clear()
            .env("PATH", &self.path)
            .env("HOME", &self.root)
            .env("STADO_CONFIG", self.root.join(".stado").join("config.json"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.root.join("store"))
            .env("OKO_DEFECT_REGISTER", self.register())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn register(&self) -> PathBuf {
        self.root.join("defects.json")
    }

    fn cli(&self, args: &[&str]) -> String {
        let output: Output = self.command().args(args).output().unwrap();
        assert!(
            output.status.success(),
            "stado {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn free_bind(&self) -> String {
        let printed = self.cli(&["host", "local", "port"]);
        match printed.trim().parse() {
            Ok(port) => SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).to_string(),
            Err(error) => panic!("stado host local port printed {printed:?}: {error}"),
        }
    }

    /// Configure the deployment so `stado serve --resolver` stops on the
    /// bind this listener holds, and answer the listener and the bind.
    pub(crate) fn broken(&self) -> (TcpListener, String) {
        self.cli(&["config", "init"]);
        let api_bind = self.free_bind();
        let mut document: Value =
            serde_json::from_str(include_str!("../service_directory/registry.json")).unwrap();
        let target = document["targets"]
            .as_array_mut()
            .and_then(|targets| targets.first_mut())
            .unwrap();
        target["hostnames"] = json!([hostname()]);
        target["service_resolver"]["api_bind"] = json!(api_bind);
        let registry = self.root.join("registry.json");
        std::fs::write(&registry, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
        self.cli(&["registry", "import", &registry.to_string_lossy()]);
        let holder = TcpListener::bind(&api_bind).expect("hold the resolution API's bind");
        (holder, api_bind)
    }

    /// One start of the service, which stops; its stderr.
    pub(crate) fn stop(&self) -> String {
        let output = self
            .command()
            .args(["serve", "--resolver", "--target", TARGET])
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(!output.status.success(), "the service started: {said}");
        said
    }

    pub(crate) fn stops(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(self.root.join(".stado/serve-stops.jsonl")).unwrap();
        text.lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// The open stado defects the deployment's register holds.
    pub(crate) fn defects(&self) -> Vec<Value> {
        let document: Value = match std::fs::read(self.register()) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({"defects": []}),
            Err(error) => panic!("{} cannot be read: {error}", self.register().display()),
        };
        document["defects"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|defect| defect["product"] == "stado")
            .cloned()
            .collect()
    }

    /// The record the running release wrote when it failed on this host,
    /// under the deployment's retained releases; none when it wrote none.
    pub(crate) fn failed_record(&self) -> Option<Value> {
        let version = self
            .root
            .join(".stado/releases/stado")
            .join(env!("CARGO_PKG_VERSION"));
        let platforms = match std::fs::read_dir(&version) {
            Ok(platforms) => platforms,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => panic!("{} cannot be read: {error}", version.display()),
        };
        platforms
            .map(|platform| platform.unwrap().path().join("failed-on-this-host.json"))
            .filter(|record| record.exists())
            .map(|record| serde_json::from_slice(&std::fs::read(&record).unwrap()).unwrap())
            .next()
    }
}

fn hostname() -> String {
    let output = Command::new("hostname").output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .to_lowercase()
}
