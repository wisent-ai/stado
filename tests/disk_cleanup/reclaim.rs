//! `space reclaim` takes a tree only when no process holds it and an earlier
//! apply found it so, unchanged; age decides nothing.

use std::fs;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::native::Native;

fn reclaim(native: &Native, mode: &str) -> Value {
    let mut args = vec![
        "space",
        "reclaim",
        "example-cleanup-host",
        "--stage",
        "build_scratch",
        mode,
    ];
    if mode == "--apply" {
        args.extend(["--reason", "ownership journey"]);
    }
    args.push("--json");
    let report = native.json(&args);
    native.observe(mode, report.clone());
    report
}

fn stage(report: &Value) -> &Value {
    report["stages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stage| stage["stage"] == "build_scratch")
        .expect("the build_scratch stage")
}

fn named(list: &Value, path: &str) -> bool {
    list.as_array()
        .unwrap()
        .iter()
        .any(|entry| entry.as_str().is_some_and(|text| text.contains(path)))
}

#[test]
fn an_unheld_tree_goes_on_the_second_apply_and_a_held_or_changed_one_stays() {
    let native = Native::new("reclaim-ownership");
    let scratch = native.home.join(".stado/build-work");
    let abandoned = scratch.join("abandoned");
    let held = scratch.join("held");
    let changing = scratch.join("changing");
    for tree in [&abandoned, &held, &changing] {
        fs::create_dir_all(tree).unwrap();
        fs::write(tree.join("output"), b"build output\n").unwrap();
    }
    // A process whose working directory is the tree: `cat` waits on its
    // stdin for as long as the test holds the pipe open.
    let mut holder = Command::new("/bin/cat")
        .current_dir(&held)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let cwd = Command::new("/usr/sbin/lsof")
        .args(["-n", "-a", "-d", "cwd", "-p", &holder.id().to_string()])
        .output()
        .unwrap();
    native.observe(
        "holder",
        serde_json::json!({
            "pid": holder.id(),
            "lsof_exit": cwd.status.code(),
            "lsof": String::from_utf8_lossy(&cwd.stdout),
        }),
    );

    let first = reclaim(&native, "--apply");
    let first_stage = stage(&first);
    assert!(
        first_stage["paths"].as_array().unwrap().is_empty(),
        "{first}"
    );
    assert!(named(&first_stage["refused"], "abandoned"), "{first}");
    assert!(named(&first_stage["refused"], "changing"), "{first}");
    assert!(!named(&first_stage["refused"], "/held"), "{first}");
    assert!(abandoned.is_dir() && held.is_dir() && changing.is_dir());

    fs::write(
        changing.join("new-output"),
        b"written between the applies\n",
    )
    .unwrap();

    let preview = reclaim(&native, "--dry-run");
    assert!(named(&stage(&preview)["paths"], "abandoned"), "{preview}");
    assert!(abandoned.is_dir(), "a dry run removed a tree");

    let second = reclaim(&native, "--apply");
    let second_stage = stage(&second);
    assert!(named(&second_stage["paths"], "abandoned"), "{second}");
    assert!(!named(&second_stage["paths"], "changing"), "{second}");
    assert!(!named(&second_stage["paths"], "/held"), "{second}");
    assert!(!abandoned.exists(), "the settled unheld tree was kept");
    assert!(held.is_dir(), "a held tree was taken");
    assert!(
        changing.is_dir(),
        "a tree written between the applies was taken"
    );

    drop(holder.stdin.take());
    holder.wait().unwrap();
}
