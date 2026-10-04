use std::fs;

use serde_json::{json, Value};

use super::native::Native;

/// A registry generation written before the janitor's settings were retired,
/// as it persisted in local storage.
fn stored_with_retired_settings(native: &Native) -> Value {
    let mut document: Value = serde_json::from_slice(&fs::read(&native.registry).unwrap()).unwrap();
    document["targets"][0]["disk_cleanup"] = json!({
        "mode": "enforce", "check_interval_seconds": 300, "low_free_gb": 8,
        "target_free_gb": 20, "max_items_per_pass": 10000,
        "max_bytes_per_pass": 21474836480_i64, "max_scan_items": 200000,
        "cleaners": {"backup_twins": {"min_age_seconds": 0}}
    });
    document["targets"][0]["memory_reclaim"] = json!({
        "mode": "enforce", "check_interval_seconds": 300, "low_free_mb": 2048,
        "target_free_mb": 4096, "high_swap_used_pct": 80, "max_repairs_per_pass": 4,
        "refuse_placement": true, "max_pass_seconds": 120,
        "repairs": {"graphical_session": {"processes": ["Safari"], "min_age_seconds": 600,
                                          "allow_graphical_session": true}}
    });
    fs::write(
        &native.registry,
        serde_json::to_vec_pretty(&document).unwrap(),
    )
    .unwrap();
    native.observe("stored prior generation", document.clone());
    document
}

#[test]
fn a_registry_write_drops_the_retired_janitor_settings() {
    let native = Native::new("retired-settings");
    let mut expected = stored_with_retired_settings(&native);
    native.success(&[
        "registry",
        "set",
        "--path",
        "targets.example-cleanup-host.pinned_only",
        "--value",
        "true",
        "--json",
    ]);
    let persisted: Value = serde_json::from_slice(&fs::read(&native.registry).unwrap()).unwrap();
    native.observe("persisted registry", persisted.clone());
    let target = expected["targets"][0].as_object_mut().unwrap();
    target.remove("disk_cleanup");
    target.remove("memory_reclaim");
    target.insert("pinned_only".to_string(), json!(true));
    assert_eq!(persisted, expected);
}

#[test]
fn a_stored_generation_with_retired_settings_still_runs_the_rule() {
    let native = Native::new("retired-settings-pass");
    stored_with_retired_settings(&native);
    let candidate = native.home.join("work/target");
    native.cache(&candidate);
    native.fill_with_user_data();
    let report = native.cleanup();
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    assert_eq!(
        report["cleaners"]["build_caches"]["deleted_items"], 1,
        "{report}"
    );
    assert!(!candidate.exists());
}

#[test]
fn the_removed_setting_verbs_are_refused() {
    let native = Native::new("removed-verbs");
    let before = fs::read(&native.registry).unwrap();
    for args in [
        &[
            "space",
            "watermark",
            "example-cleanup-host",
            "--disk-max-items-per-pass",
            "1",
        ][..],
        &["space", "cleaners", "list", "example-cleanup-host"][..],
    ] {
        let response = native.run(args);
        native.observe(
            "removed verb",
            json!({"args": args, "exit_status": response.status.code()}),
        );
        assert_eq!(response.status.code(), Some(2), "{args:?} was accepted");
    }
    assert_eq!(before, fs::read(&native.registry).unwrap());
}
