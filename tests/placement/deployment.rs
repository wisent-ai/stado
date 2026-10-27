//! One isolated deployment (its own HOME, config and local store) that every
//! placement journey runs the built `stado` against, and the registry it
//! imports: the shared directory template, where `elsewhere-api` is a fixed
//! route placed on `other-host`. Every invocation's arguments, exit status
//! and output are kept as a receipt under the deployment's root, beside the
//! source revision and the binary's digest.
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

pub(crate) const SERVICE: &str = "elsewhere-api";
pub(crate) const PLACED_ON: &str = "other-host";
pub(crate) const PROFILE: &str = "elsewhere-group";

pub(crate) struct Deployment {
    pub(crate) root: PathBuf,
}

impl Deployment {
    pub(crate) fn start() -> Self {
        let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the package sits inside the checkout")
            .to_path_buf();
        let root = repository
            .join(".build/placement")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(root.join("tmp")).unwrap();
        std::fs::create_dir_all(root.join("store")).unwrap();
        Self { root }
    }

    pub(crate) fn run(&self, args: &[&str]) -> Output {
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
        let receipt = json!({
            "source_revision": env!("STADO_SOURCE_REVISION"),
            "binary_sha256": <sha2::Sha256 as sha2::Digest>::digest(
                std::fs::read(env!("CARGO_BIN_EXE_stado")).unwrap()).to_vec(),
            "arguments": args, "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        });
        std::fs::write(
            self.root.join(format!("{}.json", uuid::Uuid::new_v4())),
            serde_json::to_vec_pretty(&receipt).unwrap(),
        )
        .unwrap();
        output
    }

    pub(crate) fn cli(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "stado {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    pub(crate) fn refused(&self, args: &[&str], said: &str) {
        let before = self.registry();
        let output = self.run(args);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "stado {args:?} was not refused");
        assert!(
            stderr.contains(said),
            "stado {args:?} did not say {said:?}: {stderr}"
        );
        assert_eq!(
            self.registry(),
            before,
            "a refused {args:?} wrote the registry"
        );
    }

    pub(crate) fn import_registry(&self) {
        let mut document: Value =
            serde_json::from_str(include_str!("../service_directory/registry.json")).unwrap();
        let target = document["targets"]
            .as_array_mut()
            .and_then(|targets| targets.first_mut())
            .unwrap();
        target["hostnames"] = json!([hostname()]);
        self.import(&document);
    }

    /// Replace the deployment's registry with `document`, through the
    /// registry's own import and its validation.
    pub(crate) fn import(&self, document: &Value) {
        let registry = self
            .root
            .join(format!("registry-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(&registry, serde_json::to_vec_pretty(document).unwrap()).unwrap();
        self.cli(&["registry", "import", &registry.to_string_lossy()]);
    }

    pub(crate) fn registry(&self) -> Value {
        serde_json::from_str(&self.cli(&["registry", "pull"])).unwrap()
    }

    pub(crate) fn write_profile(&self, name: &str, profile: &Value) -> String {
        let path = self.root.join(name);
        std::fs::write(&path, serde_json::to_vec_pretty(profile).unwrap()).unwrap();
        path.to_string_lossy().into_owned()
    }
}

fn hostname() -> String {
    let output = Command::new("hostname").output().unwrap();
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .to_lowercase()
}

/// A profile placing `services` on their placed host, each unit named as the
/// template declares it there and probed at the endpoint the directory gives
/// the placed host.
pub(crate) fn profile(document: &Value, name: &str, services: &[&str]) -> Value {
    let endpoint = document["service_directory"]["services"][SERVICE]["endpoints"][PLACED_ON]
        ["url"]
        .as_str()
        .expect("the template names the placed host's endpoint");
    let mut units = serde_json::Map::new();
    let mut probes = Vec::new();
    for service in services {
        let unit = format!("com.wisent.{service}.service");
        let path = format!("/home/example/.config/systemd/user/{unit}");
        units.insert(
            service.to_string(),
            json!({ "name": service, "unit": unit, "path": path, "kind": "systemd" }),
        );
        probes.push(json!({ "service": service, "url": format!("{endpoint}/health") }));
    }
    json!({
        "name": name,
        "services": services,
        "start_order": services,
        "stop_order": services,
        "state": [],
        "hosts": { PLACED_ON: { "units": units, "probes": probes } },
    })
}

pub(crate) fn generation(document: &Value) -> u64 {
    document["service_directory"]["generation"]
        .as_u64()
        .expect("the directory carries its generation")
}
