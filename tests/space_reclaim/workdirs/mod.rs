//! Job work directories are given back only by the `queue_workdirs` stage of
//! `space reclaim`, which keeps a tree the queue still carries or a process
//! still holds. The former root command that emptied the whole work root,
//! active jobs included, is gone: the grammar refuses it as unknown.

use std::fs;
use std::process::{Command, Stdio};

use crate::native::Native;

#[test]
fn the_former_root_command_is_refused_as_unknown_and_a_held_job_tree_stays() {
    let native = Native::new("workdirs-cutover");
    let jobs = native.home.join(".stado/work/jobs");
    let held = jobs.join("job-held");
    fs::create_dir_all(&held).unwrap();
    fs::write(held.join("artifacts.json"), b"{}\n").unwrap();
    let mut holder = Command::new("/bin/cat")
        .current_dir(&held)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();

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

    let report = native.json(&[
        "space",
        "reclaim",
        "--local",
        "--stage",
        "queue_workdirs",
        "--apply",
        "--reason",
        "workdirs cutover journey",
        "--json",
    ]);
    native.observe("queue_workdirs apply", report.clone());
    assert!(
        held.join("artifacts.json").is_file(),
        "a job tree a process still holds was removed: {report}"
    );
    let text = report.to_string();
    assert!(
        text.contains("queue_workdirs") && text.contains("queue_store_unreadable"),
        "the pass does not say the queue gave no retirement proof, so every tree was kept: {report}"
    );

    drop(holder.stdin.take());
    holder.wait().unwrap();
}
