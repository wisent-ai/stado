use std::fs;

use serde_json::{json, Value};

use super::native::Native;

fn stored_generation(native: &Native) -> Value {
    native.success(&[
        "space",
        "watermark",
        "example-cleanup-host",
        "--memory-max-pass-seconds",
        "120",
        "--json",
    ]);
    let mut document: Value = serde_json::from_slice(&fs::read(&native.registry).unwrap()).unwrap();
    // A migration starts with a prior persisted generation, not a response stub.
    // This file belongs to the fixture's real, isolated local storage backend.
    document["targets"][0]["disk_cleanup"]["max_pass_seconds"] = json!(600);
    fs::write(
        &native.registry,
        serde_json::to_vec_pretty(&document).unwrap(),
    )
    .unwrap();
    native.observe("stored prior generation", document.clone());
    document
}

fn persisted(native: &Native) -> Value {
    let document: Value = serde_json::from_slice(&fs::read(&native.registry).unwrap()).unwrap();
    native.observe("persisted registry", document.clone());
    document
}

fn without_retired_disk_field(mut document: Value) -> Value {
    document["targets"][0]["disk_cleanup"]
        .as_object_mut()
        .unwrap()
        .remove("max_pass_seconds");
    document
}

#[test]
fn stored_policy_remains_usable_and_watermark_write_completes_migration() {
    let native = Native::new("stored-policy-migration");
    let old = stored_generation(&native);
    let candidate = native.cache_root.join("eligible-before-migration");
    native.cache(&candidate);
    let report = native.cleanup();
    assert_eq!(report["cleaners"]["build_caches"]["deleted_items"], 1);
    assert!(
        !candidate.exists(),
        "the old registry prevented real reclamation"
    );
    assert_eq!(
        persisted(&native),
        old,
        "reading a policy rewrote the registry"
    );

    let target = old["targets"][0]["disk_cleanup"]["target_free_gb"]
        .as_u64()
        .unwrap()
        + 1;
    let receipt = native.json(&[
        "space",
        "watermark",
        "example-cleanup-host",
        "--disk-target-free-gb",
        &target.to_string(),
        "--json",
    ]);
    let mut expected = without_retired_disk_field(old);
    expected["targets"][0]["disk_cleanup"]["target_free_gb"] = json!(target);
    assert_eq!(persisted(&native), expected);
    assert_eq!(
        receipt["disk_cleanup"],
        expected["targets"][0]["disk_cleanup"]
    );
    assert_eq!(
        expected["targets"][0]["memory_reclaim"]["max_pass_seconds"],
        120
    );
}

#[test]
fn pushing_an_old_document_persists_only_current_fields_and_set_reports_actual_value() {
    let native = Native::new("pushed-policy-migration");
    let old = stored_generation(&native);
    let input = native.home.join("prior-registry.json");
    fs::write(&input, serde_json::to_vec_pretty(&old).unwrap()).unwrap();
    let receipt = native.json(&["registry", "push", input.to_str().unwrap(), "--json"]);
    assert_eq!(receipt["state"], "pushed");
    let mut expected = without_retired_disk_field(old.clone());
    assert_eq!(persisted(&native), expected);

    let mut supplied = old["targets"][0]["disk_cleanup"].clone();
    supplied["max_items_per_pass"] = json!(2);
    let receipt = native.json(&[
        "registry",
        "set",
        "--path",
        "targets.example-cleanup-host.disk_cleanup",
        "--value",
        &serde_json::to_string(&supplied).unwrap(),
        "--json",
    ]);
    expected["targets"][0]["disk_cleanup"]["max_items_per_pass"] = json!(2);
    assert_eq!(persisted(&native), expected);
    assert_eq!(receipt["value"], expected["targets"][0]["disk_cleanup"]);
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(input).unwrap()).unwrap(),
        old
    );
}

#[test]
fn retired_settings_unknown_keys_and_invalid_limits_never_change_the_registry() {
    let native = Native::new("migration-refusals");
    stored_generation(&native);
    let before = fs::read(&native.registry).unwrap();
    for value in ["600", "900"] {
        let response = native.run(&[
            "registry",
            "set",
            "--path",
            "targets.example-cleanup-host.disk_cleanup.max_pass_seconds",
            "--value",
            value,
            "--json",
        ]);
        assert_eq!(response.status.code(), Some(2));
        assert_eq!(fs::read(&native.registry).unwrap(), before);
    }
    let unknown = native.run(&[
        "registry",
        "set",
        "--path",
        "targets.example-cleanup-host.disk_cleanup.unrecognized_cleanup_field",
        "--value",
        "1",
        "--json",
    ]);
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("unrecognized_cleanup_field"));
    assert_eq!(fs::read(&native.registry).unwrap(), before);
    let invalid = native.run(&[
        "space",
        "watermark",
        "example-cleanup-host",
        "--disk-target-free-gb",
        "1",
        "--json",
    ]);
    assert!(!invalid.status.success());
    assert_eq!(fs::read(&native.registry).unwrap(), before);
    native.observe("refusals preserved the prior generation", json!(true));
}
