//! `stado service directory beside`: a host states that it runs a service for
//! its own callers beside the placement the fleet directory names elsewhere,
//! and `connect` on that host answers it, said as `beside`.
//!
//! One isolated deployment (its own HOME, config and local store): the shared
//! directory template with this machine as `marker-host`, `elsewhere-api`
//! placed on `other-host`, and one resolver adapter for it on this machine.
//! Expected: before any declaration, connect answers the adapter as `placed`
//! on `other-host`; a URL that is not loopback with a port is refused and
//! nothing is written; a loopback declaration is listed, kept in the host's
//! own `beside.json`, and connect answers it as `beside` on this machine; a
//! withdrawal brings back the adapter answer, and withdrawing again is
//! refused naming the service. The registry is never written.
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

const SERVICE: &str = "elsewhere-api";
const CONSUMER: &str = "marker-consumer";
const PLACED_ON: &str = "other-host";
const THIS_HOST: &str = "marker-host";

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
            .join(".build/service-directory-beside")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        std::fs::create_dir_all(root.join("store")).unwrap();
        Self { root }
    }

    fn run(&self, args: &[&str]) -> Output {
        let output = Command::new(env!("CARGO_BIN_EXE_stado"))
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
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
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

    fn free_bind(&self) -> String {
        let printed = self.cli(&["host", "local", "port"]);
        match printed.trim().parse() {
            Ok(port) => SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).to_string(),
            Err(error) => panic!("stado host local port printed {printed:?}: {error}"),
        }
    }

    fn import_registry(&self, adapter_bind: &str) {
        let mut document: Value = serde_json::from_str(include_str!("registry.json")).unwrap();
        let target = document["targets"]
            .as_array_mut()
            .and_then(|targets| targets.first_mut())
            .unwrap();
        target["hostnames"] = json!([hostname()]);
        target["service_resolver"]["adapters"] =
            json!([{"service": SERVICE, "consumer": CONSUMER, "bind": adapter_bind}]);
        let registry = self.root.join("registry.json");
        std::fs::write(&registry, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
        self.cli(&["registry", "import", &registry.to_string_lossy()]);
    }

    fn connect(&self) -> Value {
        let printed = self.cli(&[
            "service",
            "directory",
            "connect",
            SERVICE,
            "--consumer",
            CONSUMER,
            "--no-verify",
            "--json",
        ]);
        serde_json::from_str(&printed).unwrap()
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
fn a_service_declared_beside_its_placement_is_answered_on_this_host_and_said_as_such() {
    let deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    let adapter_bind = deployment.free_bind();
    deployment.import_registry(&adapter_bind);

    let placed = deployment.connect();
    assert_eq!(placed["placement"], "placed", "{placed}");
    assert_eq!(placed["placed_on"], PLACED_ON, "{placed}");
    assert_eq!(placed["url"], format!("http://{adapter_bind}"), "{placed}");

    let refused = deployment.run(&[
        "service",
        "directory",
        "beside",
        SERVICE,
        "--url",
        "http://example.com:8080",
    ]);
    let said = String::from_utf8_lossy(&refused.stderr).into_owned();
    assert!(
        !refused.status.success(),
        "a non-loopback address was accepted: {said}"
    );
    assert!(
        said.contains("must name a loopback address with its port"),
        "{said}"
    );
    assert!(
        !deployment.root.join(".stado").join("beside.json").exists(),
        "a refused declaration wrote the file"
    );

    let local = format!("http://{}", deployment.free_bind());
    let listed = deployment.cli(&[
        "service",
        "directory",
        "beside",
        SERVICE,
        "--url",
        &local,
        "--json",
    ]);
    let listed: Value = serde_json::from_str(&listed).unwrap();
    assert_eq!(listed["beside"][SERVICE]["url"], local.as_str(), "{listed}");
    let kept: Value = serde_json::from_slice(
        &std::fs::read(deployment.root.join(".stado").join("beside.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(kept[SERVICE]["url"], local.as_str(), "{kept}");

    let beside = deployment.connect();
    assert_eq!(beside["placement"], "beside", "{beside}");
    assert_eq!(beside["placed_on"], THIS_HOST, "{beside}");
    assert_eq!(beside["url"], local.as_str(), "{beside}");

    deployment.cli(&["service", "directory", "beside", SERVICE, "--withdraw"]);
    let again = deployment.connect();
    assert_eq!(again["placement"], "placed", "{again}");
    assert_eq!(again["url"], format!("http://{adapter_bind}"), "{again}");
    let withdrawn = deployment.run(&["service", "directory", "beside", SERVICE, "--withdraw"]);
    let said = String::from_utf8_lossy(&withdrawn.stderr).into_owned();
    assert!(
        !withdrawn.status.success(),
        "withdrawing an undeclared service succeeded"
    );
    assert!(
        said.contains(&format!(
            "this host declares no {SERVICE} beside its placement"
        )),
        "{said}"
    );
}
