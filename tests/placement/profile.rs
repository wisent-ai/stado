//! `stado placement profile`: declare, list, show and remove a placement
//! profile against an isolated deployment's own registry store.
//!
//! One isolated deployment (its own HOME, config and local store) imports the
//! shared directory template, where `elsewhere-api` is a fixed route placed on
//! `other-host`. Expected: a profile naming that service and only its placed
//! host is declared, the registry then holds it, the route names it, carries
//! no `managed_service`, and the directory generation advanced; `list` and
//! `show` print it. Refusals, each leaving the registry as it was: a service
//! the directory does not declare, the same service in a second profile, a
//! redeclaration dropping a held service, a profile that does not parse, and
//! removing a profile the registry does not hold. `remove` turns the route
//! back into a fixed route naming the unit on its active host and advances
//! the generation again.
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::{json, Value};

const SERVICE: &str = "elsewhere-api";
const PLACED_ON: &str = "other-host";
const PROFILE: &str = "elsewhere-group";

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
            .join(".build/placement-profile")
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

    fn cli(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "stado {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn refused(&self, args: &[&str], said: &str) {
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

    fn import_registry(&self) {
        let mut document: Value =
            serde_json::from_str(include_str!("../service_directory/registry.json")).unwrap();
        let target = document["targets"]
            .as_array_mut()
            .and_then(|targets| targets.first_mut())
            .unwrap();
        target["hostnames"] = json!([hostname()]);
        let registry = self.root.join("registry.json");
        std::fs::write(&registry, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
        self.cli(&["registry", "import", &registry.to_string_lossy()]);
    }

    fn registry(&self) -> Value {
        serde_json::from_str(&self.cli(&["registry", "pull"])).unwrap()
    }

    fn write_profile(&self, name: &str, profile: &Value) -> String {
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
fn profile(document: &Value, name: &str, services: &[&str]) -> Value {
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

fn generation(document: &Value) -> u64 {
    document["service_directory"]["generation"]
        .as_u64()
        .expect("the directory carries its generation")
}

#[test]
fn a_placement_profile_is_declared_linked_listed_refused_and_removed() {
    let deployment = Deployment::start();
    deployment.cli(&["config", "init"]);
    deployment.import_registry();
    let before = deployment.registry();

    let declared = profile(&before, PROFILE, &[SERVICE]);
    let declared = deployment.write_profile("profile.json", &declared);
    let receipt: Value = serde_json::from_str(&deployment.cli(&[
        "placement",
        "profile",
        "declare",
        "--file",
        &declared,
        "--json",
    ]))
    .unwrap();
    assert_eq!(receipt["state"], "declared", "{receipt}");
    assert_eq!(receipt["profile"], PROFILE, "{receipt}");
    let after = deployment.registry();
    let held = after["placement_profiles"]
        .as_array()
        .and_then(|profiles| profiles.iter().find(|entry| entry["name"] == PROFILE))
        .expect("the registry holds the declared profile")
        .clone();
    assert_eq!(held["services"], json!([SERVICE]), "{held}");
    let route = &after["service_directory"]["services"][SERVICE];
    assert_eq!(route["placement_profile"], PROFILE, "{route}");
    assert!(route.get("managed_service").is_none(), "{route}");
    assert_eq!(route["active_host"], PLACED_ON, "{route}");
    assert!(generation(&after) > generation(&before));

    let listed = deployment.cli(&["placement", "profile", "list", "--json"]);
    let listed: Value = serde_json::from_str(&listed).unwrap();
    let expected = json!([{ "name": PROFILE, "services": [SERVICE], "hosts": [PLACED_ON] }]);
    assert_eq!(listed, expected);
    let shown = deployment.cli(&["placement", "profile", "show", PROFILE]);
    let shown: Value = serde_json::from_str(&shown).unwrap();
    assert_eq!(shown, held);

    let unknown = profile(&before, "unknown-group", &["absent-api"]);
    let unknown = deployment.write_profile("unknown.json", &unknown);
    deployment.refused(
        &["placement", "profile", "declare", "--file", &unknown],
        "service absent-api is not in the service directory; declare it with stado service declare",
    );
    let twice = profile(&before, "second-group", &[SERVICE]);
    let twice = deployment.write_profile("twice.json", &twice);
    deployment.refused(
        &["placement", "profile", "declare", "--file", &twice],
        &format!("service {SERVICE} already moves with placement profile {PROFILE}"),
    );
    let dropping = profile(&before, PROFILE, &["marker-api"]);
    let dropping = deployment.write_profile("dropping.json", &dropping);
    deployment.refused(
        &["placement", "profile", "declare", "--file", &dropping],
        &format!(
            "placement profile {PROFILE} holds service {SERVICE}, which this declaration drops"
        ),
    );
    let mut malformed = profile(&before, "malformed-group", &["marker-api"]);
    malformed["unexpected"] = json!(true);
    let malformed = deployment.write_profile("malformed.json", &malformed);
    deployment.refused(
        &["placement", "profile", "declare", "--file", &malformed],
        "the placement profile does not parse as the registry reads one",
    );
    deployment.refused(
        &["placement", "profile", "remove", "unknown-group"],
        "the registry declares no placement profile named unknown-group",
    );

    let removed = deployment.cli(&["placement", "profile", "remove", PROFILE, "--json"]);
    let removed: Value = serde_json::from_str(&removed).unwrap();
    assert_eq!(removed["state"], "removed", "{removed}");
    let restored = deployment.registry();
    let route = &restored["service_directory"]["services"][SERVICE];
    assert!(route.get("placement_profile").is_none(), "{route}");
    assert_eq!(route["managed_service"], SERVICE, "{route}");
    let remaining = restored["placement_profiles"]
        .as_array()
        .expect("the profiles array stays");
    assert!(remaining.iter().all(|entry| entry["name"] != PROFILE));
    assert!(generation(&restored) > generation(&after));
}
