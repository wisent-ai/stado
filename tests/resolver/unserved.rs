//! A resolver that cannot serve one declared route serves every other one,
//! and names the one it does not serve where it is dialled.
//!
//! One isolated deployment, as in `tests/directory_connect/not_serving.rs`:
//! a local store holding the shared directory template with this machine as
//! `marker-host`, two adapters declared for `elsewhere-api` (placed on
//! `other-host`), and the real `stado serve --resolver` started while this
//! test holds the first adapter's bind. Expected: the resolver keeps running
//! and says it serves; the second adapter accepts connections; `stado
//! resolver status` names the held adapter with the bind error instead of
//! reporting nothing listening; and `stado service directory connect` on the
//! held adapter is refused with the resolver's pid, the bind and that error.
//! Before, one held port stopped the whole resolver and every route with it.
use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};

use serde_json::{json, Value};

const TARGET: &str = "marker-host";
const SERVICE: &str = "elsewhere-api";
const HELD: &str = "held-consumer";
const SERVED: &str = "served-consumer";
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
            .join(".build/resolver-unserved")
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
        let printed = self.cli(&["host", "free-port-local"]);
        match printed.trim().parse() {
            Ok(port) => SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).to_string(),
            Err(error) => panic!("stado host free-port-local printed {printed:?}: {error}"),
        }
    }

    /// The shared directory template with this machine as the reader, its
    /// resolution API at `api_bind` and one adapter per consumer.
    fn import_registry(&self, api_bind: &str, held_bind: &str, served_bind: &str) {
        let mut document: Value =
            serde_json::from_str(include_str!("../service_directory/registry.json")).unwrap();
        let target = document["targets"]
            .as_array_mut()
            .and_then(|targets| targets.first_mut())
            .unwrap();
        target["hostnames"] = json!([hostname()]);
        target["service_resolver"]["api_bind"] = json!(api_bind);
        target["service_resolver"]["adapters"] = json!([
            {"service": SERVICE, "consumer": HELD, "bind": held_bind},
            {"service": SERVICE, "consumer": SERVED, "bind": served_bind},
        ]);
        let registry = self.root.join("registry.json");
        std::fs::write(&registry, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
        self.cli(&["registry", "import", &registry.to_string_lossy()]);
    }

    /// Start the real resolver and return once it says it serves, reading
    /// its own stderr line by line; every line it printed is kept.
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
fn a_held_adapter_port_leaves_that_route_unserved_and_named_while_every_other_route_is_served() {
    let mut deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    let api_bind = deployment.free_bind();
    let held_bind = deployment.free_bind();
    let served_bind = deployment.free_bind();
    deployment.import_registry(&api_bind, &held_bind, &served_bind);

    let holder = TcpListener::bind(&held_bind).expect("hold the first adapter's bind");
    deployment.serve();

    TcpStream::connect(&served_bind)
        .expect("the adapter the resolver could bind accepts connections");

    let status = deployment.run(&["resolver", "status", "--target", TARGET]);
    let reported = String::from_utf8_lossy(&status.stdout).into_owned();
    assert!(
        reported.contains(&format!(
            "the resolver does not serve the {SERVICE} adapter for consumer {HELD} at {held_bind}"
        )) && reported.contains(&format!("could not bind {held_bind}")),
        "resolver status names the unserved route with its bind error: {reported}"
    );

    let output = deployment.run(&[
        "service",
        "directory",
        "connect",
        SERVICE,
        "--consumer",
        HELD,
    ]);
    let said = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        !output.status.success(),
        "a connect to the unserved route is refused: {said}"
    );
    assert!(
        said.contains(&format!("does not serve {held_bind} for {SERVICE}/{HELD}"))
            && said.contains(&format!("could not bind {held_bind}")),
        "the refusal carries the resolver's reason for that one route: {said}"
    );
    drop(holder);
}
