//! Real host measurements and real declaration refusals remain distinguishable.
use std::fs;
use std::path::Path;
use std::process::Command;

use crate::fixture::{Host, TARGET};
use crate::native::{df_root, KIB};
use serde_json::{json, Value};

// Match the existing disk-scope allowance for filesystem changes between reads.
const MEASUREMENT_DRIFT_BYTES: i64 = 4 * 1024 * 1024 * 1024;
const REFUSED: i32 = 1;

#[test]
fn a_failed_host_read_keeps_completed_store_reads_and_never_invents_disk_pressure() {
    let evidence_root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join(".wisent-output/host-diagnostics");
    fs::create_dir_all(&evidence_root).unwrap();
    let evidence = tempfile::Builder::new()
        .prefix("gates-")
        .tempdir_in(evidence_root)
        .unwrap()
        .keep();
    let revision = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    assert!(revision.status.success());
    fs::write(evidence.join("source-revision.txt"), revision.stdout).unwrap();
    let host = Host::new();
    fs::copy(
        host.storage.join("registry.json"),
        evidence.join("registry-before.json"),
    )
    .unwrap();

    let measured = run(&host, &evidence, "measured");
    assert_eq!(measured["complete"], true, "{measured}");
    let free = measured["disk"]["free_bytes"]
        .as_i64()
        .expect("a completed host read reports measured bytes");
    let native = df_root().available_kb * KIB;
    fs::write(evidence.join("native-free-bytes.txt"), native.to_string()).unwrap();
    assert!(
        (free - native).abs() < MEASUREMENT_DRIFT_BYTES,
        "reported {free}, measured {native}"
    );
    assert!(
        host.janitor_state().is_none(),
        "a diagnostic read must not run cleanup"
    );

    let binary = host.home.join(".stado/bin/stado");
    fs::remove_file(&binary).unwrap();
    let configuration_missing = run(&host, &evidence, "configuration-binary-missing");
    assert_eq!(
        configuration_missing["complete"], false,
        "{configuration_missing}"
    );
    assert!(configuration_missing["claiming"].is_null());
    let retained_free = configuration_missing["disk"]["free_bytes"]
        .as_i64()
        .unwrap();
    assert!((retained_free - df_root().available_kb * KIB).abs() < MEASUREMENT_DRIFT_BYTES);
    assert_eq!(
        observation(&configuration_missing, "disk_usage")["state"],
        "complete"
    );
    let failure = observation(&configuration_missing, "agent_store");
    assert_eq!(failure["state"], "error");
    assert!(
        failure["detail"]
            .as_str()
            .unwrap()
            .contains(binary.to_str().unwrap()),
        "{failure}"
    );
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_stado"), &binary).unwrap();

    // Withdraw only this fixture's identification of the current host. It has
    // no remote connection declaration, so the real host reader must refuse.
    // The real local object store still answers capacity and queue reads.
    let path = host.storage.join("registry.json");
    let mut registry: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    registry["targets"][0]["hostnames"] = json!(["unreachable-host.invalid"]);
    fs::write(&path, serde_json::to_vec_pretty(&registry).unwrap()).unwrap();
    let partial = run(&host, &evidence, "host-refused");
    assert_eq!(partial["complete"], false, "{partial}");
    assert!(
        partial["claiming"].is_null(),
        "an incomplete diagnosis cannot decide admission: {partial}"
    );
    assert!(partial["disk"]["free_bytes"].is_null(), "{partial}");
    assert!(partial["disk"]["below_watermark"].is_null(), "{partial}");
    assert!(partial["disk"]["pressure_source"].is_null(), "{partial}");
    assert_eq!(
        observation(&partial, "disk_usage")["state"],
        "error",
        "{partial}"
    );
    assert_eq!(
        observation(&partial, "queue")["state"],
        "complete",
        "{partial}"
    );
    assert_eq!(
        observation(&partial, "capacity")["state"],
        "absent",
        "{partial}"
    );
    assert!(!partial["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| matches!(
            value.as_str(),
            Some("disk_pressure_active" | "disk_pressure_unresolved")
        )));
    let persisted: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(
        persisted, registry,
        "a read must not repair the declaration"
    );
    fs::copy(&path, evidence.join("registry-after.json")).unwrap();

    fs::write(&path, b"{invalid registry").unwrap();
    let unavailable = run(&host, &evidence, "registry-refused");
    assert_eq!(unavailable["complete"], false, "{unavailable}");
    assert_eq!(
        observation(&unavailable, "registry")["state"],
        "error",
        "{unavailable}"
    );
    assert_eq!(
        observation(&unavailable, "disk_usage")["state"],
        "skipped",
        "{unavailable}"
    );
    assert!(unavailable["claiming"].is_null());
    assert!(unavailable["disk"]["free_bytes"].is_null());
    assert_eq!(fs::read(&path).unwrap(), b"{invalid registry");
    eprintln!("host diagnostic evidence: {}", evidence.display());
}

fn observation<'a>(report: &'a Value, operation: &str) -> &'a Value {
    report["observations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|value| value["operation"] == operation)
        .expect("named diagnostic reading")
}

fn run(host: &Host, evidence: &Path, step: &str) -> Value {
    let args = ["host", "gates", TARGET, "--json"];
    let output = host.run(&args);
    fs::write(evidence.join(format!("{step}.stdout")), &output.stdout).unwrap();
    fs::write(evidence.join(format!("{step}.stderr")), &output.stderr).unwrap();
    fs::write(
        evidence.join(format!("{step}.process.json")),
        serde_json::to_vec_pretty(&json!({
            "command": [env!("CARGO_BIN_EXE_stado"), "host", "gates", TARGET, "--json"],
            "exit_code": output.status.code(), "success": output.status.success(),
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        output.status.code(),
        Some(REFUSED),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("a diagnostic refusal retains a JSON report")
}
