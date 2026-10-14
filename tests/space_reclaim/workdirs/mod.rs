//! Job work directories are given back only by the `queue_workdirs` cleaner
//! of `space reclaim` (the local janitor pass, or the fleet stage of that
//! name), which takes a tree only when the queue says its job is retired and
//! keeps it, with the reason, when the queue still carries the job or could
//! not be asked. The former root command that emptied the whole work root,
//! active jobs included, is gone: the grammar refuses it as unknown.

use std::fs;

use crate::native::Native;

#[test]
fn the_former_root_command_is_refused_and_the_queue_decides_a_job_tree() {
    let native = Native::new("workdirs-cutover");
    let jobs = native.home.join(".stado/work/jobs");
    let held = jobs.join("wc-job-0123456789abcdef01234567");
    fs::create_dir_all(&held).unwrap();
    fs::write(held.join("artifacts.json"), b"{}\n").unwrap();

    let refused = native.run(&["workdirs", "--apply", "--json"]);
    let usage = String::from_utf8_lossy(&refused.stderr).to_string();
    native.observe(
        "former root command",
        serde_json::json!({"status": refused.status.code(), "stderr": usage}),
    );
    assert!(
        !refused.status.success(),
        "the retired root command still runs: {usage}"
    );
    assert!(
        usage.contains("unrecognized subcommand") && usage.contains("workdirs"),
        "the refusal names the unknown verb: {usage}"
    );

    let refused_stage = native.run(&["space", "reclaim", "--local", "--stage", "queue_workdirs"]);
    let stage_usage = String::from_utf8_lossy(&refused_stage.stderr).to_string();
    assert!(
        !refused_stage.status.success() && stage_usage.contains("require TARGET"),
        "a local pass selecting a fleet stage is refused naming the rule: {stage_usage}"
    );

    let user_data = native.fill_with_user_data();
    let report = native.cleanup();
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    let cleaner = &report["cleaners"]["queue_workdirs"];
    assert!(
        cleaner.is_object(),
        "the pass does not report the queue_workdirs cleaner: {report}"
    );
    let trees = [held.as_path()];
    let count = |field: &str| cleaner[field].as_u64().map(|n| n as usize);
    assert_eq!(count("scanned_items"), Some(trees.len()), "{cleaner}");
    // The queue decides the tree's fate: a job it has retired goes and is
    // counted as deleted; one it still carries, or one it could not be asked
    // about, stays and is counted under its skip reason. The count and the
    // filesystem must agree either way.
    let deleted = count("deleted_items") == Some(trees.len());
    assert_eq!(
        held.exists(),
        !deleted,
        "the cleaner's count disagrees with the tree: {cleaner}"
    );
    if !deleted {
        assert!(
            cleaner["skipped"]
                .as_object()
                .is_some_and(|skipped| !skipped.is_empty()),
            "a kept tree carries no skip reason: {cleaner}"
        );
    }
    assert!(user_data.is_file(), "the user's data was deleted");
}
