//! Real installed Stado binaries, the host channel, and the native HTTP API.
//! Only the fixture's home and configuration change; no service is reconciled.
use super::fixture::{binary_digest, Service};
use super::{confirmed, payload};
use serde_json::{json, Value};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub(super) fn confined_storage(home: &Path, value: &Value) {
    let raw = value.as_str().expect("resolved local storage path");
    let path = raw
        .strip_prefix("~/")
        .map_or_else(|| PathBuf::from(raw), |part| home.join(part));
    assert!(
        path.is_absolute() && path.starts_with(home),
        "storage escapes fixture: {raw}"
    );
    assert!(!path
        .components()
        .any(|part| matches!(part, Component::ParentDir)));
}

fn prepare_host(service: &mut Service, binary: &Path, explicit_json: bool) -> (String, PathBuf) {
    let controller: Value =
        serde_json::from_str(&service.cli(&["config", "show", "--json"])).unwrap();
    assert_eq!(controller["file"], service.config.to_str().unwrap());
    assert_eq!(controller["resolved"]["wc_storage_backend"], "local");
    assert_eq!(controller["resolved"]["wc_backup_storage_backend"], "local");
    confined_storage(
        &service.home,
        &controller["resolved"]["wc_local_storage_path"],
    );
    confined_storage(
        &service.home,
        &controller["resolved"]["wc_backup_local_storage_path"],
    );
    service.observe("controller_configuration", controller);

    let installed = service.home.join(".stado/bin/stado");
    fs::create_dir_all(installed.parent().unwrap()).unwrap();
    fs::copy(binary, &installed).expect("stage the genuine installed-host executable");
    let version = service.execute(&installed, &["--version"]);
    assert!(version.status.success());
    service.observe(
        "host_binary",
        json!({
            "source": binary, "installed": installed, "sha256": binary_digest(&installed),
            "version": String::from_utf8(version.stdout).unwrap(), "explicit_json": explicit_json,
        }),
    );
    let probe = service.execute(&installed, &["config", "show", "--json"]);
    let machine = if explicit_json {
        assert!(
            probe.status.success(),
            "the current host must support explicit JSON"
        );
        probe
    } else {
        assert_eq!(
            probe.status.code(),
            Some(2),
            "supply a real host binary that predates --json"
        );
        let bare = service.execute(&installed, &["config", "show"]);
        assert!(bare.status.success());
        bare
    };
    let native: Value = serde_json::from_slice(&machine.stdout).unwrap();
    assert_eq!(native["file"], service.config.to_str().unwrap());
    assert!(native["resolved"].is_object());

    let configuration = service.home.join(".config/stado/config.json");
    fs::create_dir_all(configuration.parent().unwrap()).unwrap();
    fs::copy(&service.config, &configuration).unwrap();
    let registry: Value = serde_json::from_str(&service.cli(&["registry", "pull"])).unwrap();
    assert_eq!(registry["targets"].as_array().unwrap().len(), 1);
    assert_eq!(registry["targets"][0]["kind"], "local");
    let host = registry["targets"][0]["name"].as_str().unwrap().to_owned();
    service.observe("isolated_target", registry["targets"][0].clone());
    (host, configuration)
}

fn document(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn readback(value: &Value, configuration: &Path) {
    assert_eq!(value["file"], configuration.to_str().unwrap());
    assert!(value["resolved"].is_object());
}

async fn configuration_lifecycle(mut service: Service, binary: &Path, explicit_json: bool) {
    let (host, configuration) = prepare_host(&mut service, binary, explicit_json);
    let shown: Value =
        serde_json::from_str(&service.cli(&["host", "config", "show", &host, "--json"])).unwrap();
    readback(&shown, &configuration);
    let request = json!({"args": ["host", "config", "show", host, "--json"]});
    let api = payload(service.call(request.clone(), 200).await);
    assert_eq!(api, shown);

    let set = json!({"args": ["host", "config", "set", host, "providers_disabled", "[]"]});
    let before = fs::read(&configuration).unwrap();
    service.call(set.clone(), 403).await;
    assert_eq!(
        fs::read(&configuration).unwrap(),
        before,
        "unreviewed write changed the host"
    );
    let azure = "[\"azure\"]";
    let changed = service.cli(&["host", "config", "set", &host, "providers_disabled", azure]);
    let changed: Value = serde_json::from_str(&changed).unwrap();
    readback(&changed, &configuration);
    assert_eq!(
        document(&configuration)["providers_disabled"],
        json!(["azure"])
    );
    service.observe("cli_write", document(&configuration));
    readback(
        &payload(service.call(confirmed(set), 200).await),
        &configuration,
    );
    assert_eq!(document(&configuration)["providers_disabled"], json!([]));
    service.observe("api_write", document(&configuration));

    let before = fs::read(&configuration).unwrap();
    let stado = Path::new(env!("CARGO_BIN_EXE_stado"));
    let key = "providers_disabled";
    let refusal = service.execute(stado, &["host", "config", "set", &host, key, "{}"]);
    assert!(!refusal.status.success());
    assert_eq!(
        fs::read(&configuration).unwrap(),
        before,
        "invalid input changed the host"
    );
    let diagnostic = format!(
        "{}\n{}",
        String::from_utf8_lossy(&refusal.stdout),
        String::from_utf8_lossy(&refusal.stderr)
    );
    assert!(diagnostic.contains(&host) && diagnostic.contains("config set providers_disabled"));

    let unset = json!({"args": ["host", "config", "unset", host, "providers_disabled"]});
    service.call(unset.clone(), 403).await;
    assert_eq!(fs::read(&configuration).unwrap(), before);
    readback(
        &payload(service.call(confirmed(unset), 200).await),
        &configuration,
    );
    assert!(document(&configuration).get("providers_disabled").is_none());
    let repeated = service.cli(&["host", "config", "unset", &host, "providers_disabled"]);
    let repeated: Value = serde_json::from_str(&repeated).unwrap();
    readback(&repeated, &configuration);
    assert!(document(&configuration).get("providers_disabled").is_none());
    service.observe("final_host_configuration", document(&configuration));

    // A real malformed file must remain a refusal, not an empty successful read.
    fs::write(&configuration, b"{").unwrap();
    let refused = service.call(request, 200).await;
    assert_eq!(refused["ok"], false);
    assert_ne!(
        refused["exit_code"].as_i64().expect("actual child status"),
        0
    );
    let diagnostic = format!("{}\n{}", refused["stdout"], refused["stderr"]);
    assert!(diagnostic.contains(&host) && diagnostic.contains("config show --json"));
    assert!(diagnostic.contains("~/.stado/bin/stado"));
    assert_eq!(fs::read(&configuration).unwrap(), b"{");
    service.observe(
        "refused_host_configuration",
        json!({"path": configuration, "body": "{"}),
    );
    service.pass();
}

#[tokio::test]
async fn current_installed_host_configuration_cli_and_api() {
    configuration_lifecycle(
        Service::start(),
        Path::new(env!("CARGO_BIN_EXE_stado")),
        true,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires STADO_HOST_CONFIG_LEGACY_BINARY pointing to a genuine prior same-platform Stado executable"]
async fn prior_installed_host_configuration_cli_and_api() {
    let mut service = Service::start();
    let binary = service.input("STADO_HOST_CONFIG_LEGACY_BINARY");
    configuration_lifecycle(service, Path::new(&binary), false).await;
}
