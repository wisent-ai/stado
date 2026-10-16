//! A janitor pass on a full volume walks the build caches whatever the
//! host's load, since that walk is what frees the disk; a preview stays
//! bound by the load ceiling and says so.

use serde_json::Value;

use crate::native::Native;

/// The one-process form of `host.scan_load_ceiling`.
const CEILING: &str = "WC_HOST_SCAN_LOAD_CEILING";

/// `space reclaim --local` with the host's declared load ceiling set to the
/// smallest positive load average, so any working host is above it.
fn reclaim_above_ceiling(native: &Native, args: &[&str]) -> Value {
    let ceiling = f64::MIN_POSITIVE.to_string();
    let output = native
        .command(args)
        .env(CEILING, &ceiling)
        .output()
        .unwrap();
    native.record(env!("CARGO_BIN_EXE_stado"), args, &output);
    assert!(
        output.status.success(),
        "stado {args:?} exited {:?}: {}\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    native.observe(&args.join(" "), report.clone());
    report
}

fn skipped_for_load(report: &Value) -> bool {
    report["cleaners"]["build_caches"]["skipped"]
        .get("load_above_ceiling")
        .is_some()
}

#[test]
fn a_full_volume_is_walked_above_the_load_ceiling_and_a_preview_is_not() {
    let native = Native::new("walk-under-load");
    let cache = native.home.join("work/repo/target");
    native.cache(&cache);
    native.fill_with_user_data();

    let preview = reclaim_above_ceiling(&native, &["space", "reclaim", "--local", "--json"]);
    assert!(skipped_for_load(&preview), "{preview}");
    assert!(cache.is_dir(), "a preview removed the cache");

    let apply = reclaim_above_ceiling(
        &native,
        &[
            "space",
            "reclaim",
            "--local",
            "--apply",
            "--reason",
            "walk under load",
            "--json",
        ],
    );
    assert!(!skipped_for_load(&apply), "{apply}");
    assert!(
        !cache.exists(),
        "the pass at the threshold left the tagged cache: {apply}"
    );
}
