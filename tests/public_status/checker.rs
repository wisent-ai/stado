//! The published status names the machine that checked it by the kernel's
//! own name, whatever the environment says.
//!
//! Machine-name readers once ran the `hostname` program, answered an empty
//! name when it failed, and preferred `$HOSTNAME` over the kernel's name, so
//! a host read as another. `stado availability check` writes `checked_by`
//! through the one reader that asks the kernel; run here with `HOSTNAME`
//! naming another machine and with no `hostname` program on its `PATH`, it
//! must still publish the name `uname -n` reads from the same kernel.
use std::process::Command;

use crate::deployment::Deployment;

#[test]
fn the_checker_is_named_by_the_kernel_not_the_environment() {
    let kernel = Command::new("/usr/bin/uname")
        .arg("-n")
        .output()
        .expect("uname reads the kernel's node name");
    assert!(kernel.status.success(), "{kernel:?}");
    let kernel = String::from_utf8_lossy(&kernel.stdout).trim().to_owned();
    assert!(!kernel.is_empty(), "the kernel names this machine");
    let elsewhere = format!("not-{kernel}");

    let mut deployment = Deployment::start("checker");
    let mut command = deployment.command();
    command
        .env("HOSTNAME", &elsewhere)
        .env("PATH", deployment.root.join("no-programs"))
        .args(["availability", "check", "--json"]);
    let output = command.output().unwrap();
    deployment.report["commands"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "args": ["availability", "check", "--json"],
            "environment": {"HOSTNAME": elsewhere, "PATH": deployment.root.join("no-programs")},
            "exit_status": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }));
    deployment.save();
    assert!(output.status.success(), "{output:?}");
    let published: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        published["checked_by"], kernel,
        "the checker is not the kernel's name for this machine: {published}"
    );
    let shown = deployment.json(&["availability", "show", "--json"]);
    assert_eq!(shown["checked_by"], kernel, "{shown}");
    deployment.passed();
}
