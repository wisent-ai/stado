//! Routing capability tests against the real local storage and filesystem paths.

use std::path::Path;
use std::process::{Command, Output};

fn this_host() -> String {
    String::from_utf8(
        Command::new("hostname")
            .output()
            .expect("hostname runs")
            .stdout,
    )
    .expect("hostname is UTF-8")
    .trim()
    .to_ascii_lowercase()
}

fn platform() -> String {
    format!(
        "{}-{}",
        match std::env::consts::OS {
            "macos" => "darwin",
            other => other,
        },
        match std::env::consts::ARCH {
            "aarch64" => "arm64",
            other => other,
        }
    )
}

fn registry_document() -> serde_json::Value {
    serde_json::json!({
        "schema_version": 2,
        "targets": [{
            "name": "route-host",
            "kind": "local",
            "ssh": "nobody@127.0.0.1",
            "release_platform": platform(),
            "hostnames": [this_host()],
            "services": [],
            "weles": {
                "enabled": true,
                "actions": ["generic_capture", "inspect_page"]
            },
            "mobile_runtime": {
                "appium": "3.2.1",
                "drivers": ["xcuitest"],
                "platform_tools": false
            }
        }, {
            "name": "other-host",
            "kind": "local",
            "ssh": "nobody@192.0.2.2",
            "release_platform": platform(),
            "hostnames": ["other-host.invalid"],
            "services": []
        }],
        "coordinators": [],
        "service_directory": {
            "authority": {
                "target": "route-host",
                "command": env!("CARGO_BIN_EXE_stado")
            },
            "generation": 7,
            "services": {
                "weles-admission": {
                    "active_host": "route-host",
                    "endpoints": {
                        "route-host": {"url": "http://127.0.0.1:48123"},
                        "other-host": {"url": "http://127.0.0.1:49123"}
                    },
                    "consumers": {}
                },
                "skarbiec": {
                    "active_host": "route-host",
                    "endpoints": {
                        "route-host": {"url": "http://127.0.0.1:48234"}
                    },
                    "consumers": {}
                }
            }
        }
    })
}

struct Fleet {
    home: tempfile::TempDir,
    storage: tempfile::TempDir,
}

impl Fleet {
    fn new(document: &serde_json::Value) -> Self {
        let fleet = Self {
            home: tempfile::tempdir().expect("temp HOME"),
            storage: tempfile::tempdir().expect("temp storage"),
        };
        std::fs::write(
            fleet.storage.path().join("registry.json"),
            format!("{}\n", serde_json::to_string_pretty(document).unwrap()),
        )
        .unwrap();
        fleet
    }

    fn stado(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(args)
            .env("HOME", self.home.path())
            .env("WC_STORAGE_BACKEND", "local")
            .env("WC_LOCAL_STORAGE_PATH", self.storage.path())
            .env(
                "STADO_CONFIG",
                self.storage.path().join("no-such-config.json"),
            )
            .env_remove("COMPUTE_API_KEY")
            .env_remove("COMPUTE_API_URL")
            .env_remove("WC_PROFILES_DIR")
            .output()
            .expect("stado binary runs")
    }

    fn registry(&self) -> serde_json::Value {
        serde_json::from_str(
            &std::fs::read_to_string(self.storage.path().join("registry.json")).unwrap(),
        )
        .unwrap()
    }

    fn marker(&self, service: &str) -> std::path::PathBuf {
        self.home
            .path()
            .join(".stado")
            .join("forwards")
            .join(format!("{service}.local"))
    }
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn list_reads_every_endpoint_only_from_the_service_directory() {
    let declared = registry_document();
    let fleet = Fleet::new(&declared);
    let output = fleet.stado(&["route", "list", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(report["authority"]["target"], "route-host");
    assert_eq!(report["authority"]["command"], env!("CARGO_BIN_EXE_stado"));
    let routes = report["services"].as_array().unwrap();
    assert_eq!(routes.len(), 2);
    let weles = routes
        .iter()
        .find(|route| route["service"] == "weles-admission")
        .unwrap();
    assert_eq!(weles["active_host"], "route-host");
    assert_eq!(weles["authority"]["target"], "route-host");
    let endpoints = weles["endpoints"].as_array().unwrap();
    let endpoint = |target: &str| {
        endpoints
            .iter()
            .find(|endpoint| endpoint["target"] == target)
            .and_then(|endpoint| endpoint["url"].as_str())
    };
    assert_eq!(endpoint("other-host"), Some("http://127.0.0.1:49123"));
    assert_eq!(endpoint("route-host"), Some("http://127.0.0.1:48123"));
    assert_eq!(
        fleet.registry(),
        declared,
        "listing never rewrites the declaration"
    );
}

#[test]
fn unknown_service_is_refused_with_the_declaration_to_edit() {
    let declared = registry_document();
    let fleet = Fleet::new(&declared);
    let output = fleet.stado(&["route", "open", "missing", "--local"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains(
            "missing is not in the service directory; add it to service_directory.services"
        ),
        "{}",
        stderr(&output)
    );
    assert_eq!(fleet.registry(), declared);
}

#[test]
fn missing_directory_authority_is_refused_with_the_declaration_to_edit() {
    let mut declared = registry_document();
    declared["service_directory"]
        .as_object_mut()
        .unwrap()
        .remove("authority");
    let fleet = Fleet::new(&declared);
    let output = fleet.stado(&["route", "list", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains(
            "the service directory declares no authority; add it to service_directory.authority"
        ),
        "{}",
        stderr(&output)
    );
    assert_eq!(fleet.registry(), declared);
}

#[test]
fn service_without_an_endpoint_for_the_target_is_refused() {
    let declared = registry_document();
    let fleet = Fleet::new(&declared);
    let output = fleet.stado(&[
        "route",
        "open",
        "weles-admission",
        "--target",
        "missing-host",
        "--local",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains(
            "weles-admission declares no endpoint for missing-host; add it to service_directory.services.weles-admission.endpoints.missing-host"
        ),
        "{}",
        stderr(&output)
    );
    assert!(!fleet.marker("weles-admission").exists());
}

#[test]
fn open_requires_an_explicit_direction() {
    let fleet = Fleet::new(&registry_document());
    let output = fleet.stado(&["route", "open", "weles-admission"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("route open requires --local or --remote"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn opening_then_closing_leaves_no_forward_file() {
    let declared = registry_document();
    let fleet = Fleet::new(&declared);
    let opened = fleet.stado(&[
        "route",
        "open",
        "weles-admission",
        "--target",
        "other-host",
        "--local",
        "--json",
    ]);
    assert!(opened.status.success(), "{}", stderr(&opened));
    let marker = fleet.marker("weles-admission");
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap(),
        "http://127.0.0.1:49123\n",
        "the credential bridge's exact one-line .local marker comes from the directory endpoint"
    );
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&marker).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(marker.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let listed = fleet.stado(&["route", "list", "--json"]);
    let report: serde_json::Value = serde_json::from_str(&stdout(&listed)).unwrap();
    let route = report["services"]
        .as_array()
        .unwrap()
        .iter()
        .find(|route| route["service"] == "weles-admission")
        .unwrap();
    assert_eq!(route["local_forward"]["url"], "http://127.0.0.1:49123");

    let closed = fleet.stado(&[
        "route",
        "close",
        "weles-admission",
        "--target",
        "other-host",
    ]);
    assert!(closed.status.success(), "{}", stderr(&closed));
    assert!(
        !fleet.marker("weles-admission").exists(),
        "close cannot leave the credential bridge reading a stale endpoint"
    );
    assert_eq!(fleet.registry(), declared);
}

#[test]
fn remote_open_writes_the_same_declared_marker_shape_on_the_selected_host() {
    let fleet = Fleet::new(&registry_document());
    let opened = fleet.stado(&[
        "route",
        "open",
        "skarbiec",
        "--target",
        "route-host",
        "--remote",
        "--json",
    ]);
    assert!(
        opened.status.success(),
        "{}\n{}",
        stdout(&opened),
        stderr(&opened)
    );
    assert_eq!(
        std::fs::read_to_string(fleet.marker("skarbiec")).unwrap(),
        "http://127.0.0.1:48234\n"
    );
    let closed = fleet.stado(&["route", "close", "skarbiec"]);
    assert!(closed.status.success(), "{}", stderr(&closed));
    assert!(!fleet.marker("skarbiec").exists());
}

#[test]
fn capability_lookup_resolves_the_directory_active_host() {
    let fleet = Fleet::new(&registry_document());
    let output = fleet.stado(&["route", "capability", "weles-admission", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("route-host: no Skarbiec binary at")
            && stderr(&output).contains(".stado/bin/skarbiec"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn resolver_key_reads_the_declared_authority() {
    let fleet = Fleet::new(&registry_document());
    let output = fleet.stado(&["route", "key", "route-host", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains(
            "\"route-host\" IS the service-directory authority, so its resolver reads the canonical store directly and opens no session to authorize"
        ),
        "{}",
        stderr(&output)
    );
}
#[test]
fn placement_publish_writes_the_declared_policy_document() {
    let declared = registry_document();
    let fleet = Fleet::new(&declared);
    let output = fleet.stado(&["route", "placement", "publish", "--mobile", "--json"]);
    assert!(
        output.status.success(),
        "{}\n{}",
        stdout(&output),
        stderr(&output)
    );
    let report: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(report["published"].as_array().unwrap().len(), 1);
    assert_eq!(report["published"][0]["target"], "route-host");

    let installed: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            fleet
                .home
                .path()
                .join(".config/weles/placement-policy.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(installed["schema_version"], 1);
    assert_eq!(installed["hosts"][0]["hostname"], "route-host");
    assert_eq!(installed["hosts"][0]["enabled"], true);
    assert_eq!(
        installed["hosts"][0]["actions"],
        serde_json::json!(["generic_capture", "inspect_page"])
    );
    assert_eq!(installed["_source"]["by"], "stado route placement publish");
    assert_eq!(
        fleet.registry(),
        declared,
        "publication reads but never rewrites the registry"
    );
}

#[test]
fn replaced_verbs_are_absent_from_host_help() {
    let fleet = Fleet::new(&registry_document());
    let output = fleet.stado(&["host", "--help"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let help = stdout(&output);
    for retired in [
        "forward-local",
        "forward-remote",
        "forward-close",
        "capability-route",
        "resolver-key",
        "publish-placement-policy",
        "mobile-placement",
    ] {
        assert!(
            !help.contains(retired),
            "host --help still contains {retired}:\n{help}"
        );
    }
}
