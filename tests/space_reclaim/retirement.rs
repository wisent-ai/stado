use std::fs;

use serde_json::{json, Value};

use super::native::Native;

/// The status clap exits with when it refuses the command line:
/// https://docs.rs/clap/latest/clap/error/struct.Error.html#method.exit
const USAGE_STATUS: i32 = 2;

/// The one target of the isolated registry.
fn only_target(document: &mut Value) -> &mut Value {
    match document["targets"].as_array_mut().unwrap().as_mut_slice() {
        [target] => target,
        targets => panic!("the isolated registry holds {} targets", targets.len()),
    }
}

/// A registry generation written before the retired memory setting was
/// removed, as persisted in local storage. The product reads none of the
/// block, so it carries the setting's words and not its figures.
fn stored_with_retired_settings(native: &Native) -> Value {
    let mut document: Value = serde_json::from_slice(&fs::read(&native.registry).unwrap()).unwrap();
    only_target(&mut document)["memory_reclaim"] = json!({
        "mode": "enforce", "refuse_placement": true,
        "repairs": {"graphical_session": {"processes": ["Safari"],
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
    let target = only_target(&mut expected).as_object_mut().unwrap();
    target.remove("memory_reclaim");
    target.insert("pinned_only".to_string(), json!(true));
    assert_eq!(persisted, expected);
}

#[test]
fn a_stored_generation_with_retired_settings_still_runs_the_rule() {
    let native = Native::new("retired-settings-pass");
    stored_with_retired_settings(&native);
    let candidates = [native.home.join("work/target")];
    for candidate in &candidates {
        native.cache(candidate);
    }
    native.fill_with_user_data();
    let report = native.cleanup();
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    assert_eq!(
        report["cleaners"]["build_caches"]["deleted_items"],
        candidates.len(),
        "{report}"
    );
    assert!(candidates.iter().all(|candidate| !candidate.exists()));
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
        assert_eq!(
            response.status.code(),
            Some(USAGE_STATUS),
            "{args:?} was accepted"
        );
    }
    assert_eq!(before, fs::read(&native.registry).unwrap());
}

/// A turn the janitor asked running workloads for while the volume was full
/// is withdrawn by a pass that finds it below the threshold: no such pass
/// takes the turn, so a standing request would refuse every claim.
#[test]
fn a_pass_below_the_threshold_withdraws_the_turn_it_asked_for() {
    let native = Native::new("stale-turn");
    let state_dir = native.state_dir();
    fs::create_dir_all(&state_dir).unwrap();
    let turn = state_dir.join("disk-cleanup-turn.json");
    let request = json!({
        "reason": "the volume was at the disk-full threshold",
        "pid": std::process::id(),
    });
    fs::write(&turn, request.to_string()).unwrap();
    let report = native.cleanup();
    assert_eq!(report["rule"]["triggered"], false, "{report}");
    assert_eq!(report["outcome"], "healthy_noop", "{report}");
    native.observe("turn after the pass", json!({"present": turn.exists()}));
    assert!(
        !turn.exists(),
        "a pass below the threshold left the turn request standing"
    );
}
