//! `stado service directory connect <service>` on a machine whose own resolver
//! stopped names the resolver's published reason beside the refusal, so a
//! connection refused on the adapter's loopback bind is read as the resolver
//! holding no listener, not as the placed service failing on its host.
//!
//! One isolated deployment: a local store holding the shared directory
//! template with this machine as `marker-host` and an adapter declared for
//! `elsewhere-api` (placed on `other-host`), and the real `stado serve
//! --resolver` run while this test holds the resolution API's bind, so the
//! resolver publishes `failed` with the bind error and exits. The connect
//! that follows is refused with that reason, the pid that published it and
//! the state it published.
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

const TARGET: &str = "marker-host";
const SERVICE: &str = "elsewhere-api";
const CONSUMER: &str = "marker-consumer";
const FAILED: &str = "failed";
const NO_LISTENER: &str = "holds no listener";

struct Deployment {
    root: PathBuf,
}

impl Deployment {
    fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the package sits inside the checkout")
            .to_path_buf();
        let root = repository
            .join(".build/directory-connect-not-serving")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        std::fs::create_dir_all(root.join("store")).unwrap();
        Self { root }
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

    fn run(&self, args: &[&str]) -> Output {
        let output = self.command().args(args).output().unwrap();
        let name = args.join("-").replace('/', "_");
        std::fs::write(self.root.join(format!("{name}.stdout")), &output.stdout).unwrap();
        std::fs::write(self.root.join(format!("{name}.stderr")), &output.stderr).unwrap();
        output
    }

    fn cli(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "stado {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// A loopback address this machine just handed out and nothing holds now.
    fn free_bind(&self) -> String {
        let printed = self.cli(&["host", "local", "port"]);
        match printed.trim().parse() {
            Ok(port) => SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).to_string(),
            Err(error) => panic!("stado host local port printed {printed:?}: {error}"),
        }
    }

    /// The shared directory template with this machine as the reader, its
    /// resolution API at `api_bind` and one adapter for the service placed
    /// elsewhere at `adapter_bind`.
    fn import_registry(&self, api_bind: &str, adapter_bind: &str) {
        let mut document: Value =
            serde_json::from_str(include_str!("../service_directory/registry.json")).unwrap();
        let target = document["targets"]
            .as_array_mut()
            .and_then(|targets| targets.first_mut())
            .unwrap();
        target["hostnames"] = json!([hostname()]);
        target["service_resolver"]["api_bind"] = json!(api_bind);
        target["service_resolver"]["adapters"] =
            json!([{"service": SERVICE, "consumer": CONSUMER, "bind": adapter_bind}]);
        let registry = self.root.join("registry.json");
        std::fs::write(&registry, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
        self.cli(&["registry", "import", &registry.to_string_lossy()]);
    }
}

impl Drop for Deployment {
    fn drop(&mut self) {
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
fn a_connect_refused_on_this_hosts_adapter_names_the_resolvers_published_reason() {
    let deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    let api_bind = deployment.free_bind();
    let adapter_bind = deployment.free_bind();
    deployment.import_registry(&api_bind, &adapter_bind);

    // The resolution API's bind is held here, so the real resolver cannot
    // bind it, publishes `failed` with the bind error and exits.
    let holder = TcpListener::bind(&api_bind).expect("hold the resolution API's bind");
    let served = deployment.run(&["serve", "--resolver", "--target", TARGET]);
    let served_said = String::from_utf8_lossy(&served.stderr).into_owned();
    assert!(
        !served.status.success(),
        "the resolver whose API bind is held exits with the bind error: {served_said}"
    );
    drop(holder);

    let output = deployment.run(&[
        "service",
        "directory",
        "connect",
        SERVICE,
        "--consumer",
        CONSUMER,
    ]);
    let said = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        !output.status.success(),
        "a connect nobody answers on the adapter is refused: {said}"
    );
    assert!(
        said.contains(&format!("{SERVICE} is placed on")) && said.contains(&adapter_bind),
        "the refusal names the service's placement and the adapter it dialled: {said}"
    );
    assert!(
        said.contains(NO_LISTENER) && said.contains(&format!("state {FAILED}")),
        "the refusal carries the resolver's published state: {said}"
    );
    assert!(
        said.contains(&api_bind),
        "the resolver's own reason, the bind it could not take, is in the refusal: {said}"
    );
}
