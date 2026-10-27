//! The disk-full rule is judged on one host by two commands, and they must
//! agree: `stado host gates --require-disk` and `stado space report` both
//! read the fleet volume's `df -Pk` row through the one reader in
//! `deploy::host_disk`. An isolated deployment whose registry names this
//! machine has no capacity publication, so `host gates` measures the volume
//! itself rather than repeating an agent's word, and `space report` measures
//! it too. Both must have measured it, give the same verdict, and the exit
//! status of `--require-disk` must follow that verdict: success under the
//! threshold, a refusal naming the used percentage at or past it.
use super::deployment::Deployment;
use serde_json::Value;
use std::fs;
use std::process::Output;

/// A command's standard output as the JSON document it promises, or a panic
/// naming the command's own streams.
fn document(what: &str, output: &Output) -> Value {
    match serde_json::from_slice(&output.stdout) {
        Ok(value) => value,
        Err(error) => panic!(
            "{what} printed no JSON document ({error}); stdout {:?}, stderr {:?}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    }
}

#[test]
fn host_gates_and_space_report_judge_the_disk_rule_alike() {
    let mut deployment = Deployment::start();
    let registry: Value =
        serde_json::from_slice(&fs::read(deployment.store().join("registry.json")).unwrap())
            .unwrap();
    let Some(host) = registry["targets"]
        .as_array()
        .and_then(|targets| targets.first())
        .and_then(|target| target["name"].as_str())
        .map(str::to_string)
    else {
        panic!("config init must seed a registry naming this machine: {registry}");
    };

    let gates_run = deployment.run(&["host", "gates", &host, "--json", "--require-disk"]);
    let gates = document("stado host gates --json", &gates_run);
    let disk = &gates["disk"];
    assert_eq!(
        disk["pressure_source"], "host_disk_measurement",
        "with no capacity publication the command must measure the volume itself: {disk}"
    );
    let Some(full) = disk["full"].as_bool() else {
        panic!("host gates measured no used percentage: {disk}");
    };
    assert!(
        disk["local_disk_unread"].is_null(),
        "a volume host gates measured carries no unread cause: {disk}"
    );

    let report_run = deployment.run(&["space", "report", &host, "--json"]);
    let report = document("stado space report --json", &report_run);
    let coverage = &report["coverage"];
    let Some(verdict) = coverage["verdict"].as_str() else {
        panic!("space report carries no coverage verdict: {coverage}");
    };
    assert_ne!(
        verdict, "unmeasured",
        "space report must measure the volume host gates measured: {}",
        coverage["detail"]
    );
    assert_eq!(
        verdict == "full",
        full,
        "the two commands judge the disk-full rule differently on {host}: host gates {disk}, space report {coverage}"
    );
    assert_eq!(
        coverage["rule"]["triggered"].as_bool(),
        Some(full),
        "the coverage rule disagrees with its own verdict: {coverage}"
    );

    if full {
        assert!(
            !gates_run.status.success(),
            "--require-disk must refuse a volume at or past the threshold: {disk}"
        );
        let said = String::from_utf8_lossy(&gates_run.stderr);
        assert!(
            said.contains("disk-full threshold") && said.contains("% used"),
            "the refusal names the used percentage and the threshold: {said}"
        );
    } else {
        assert!(
            gates_run.status.success(),
            "--require-disk must admit a volume under the threshold: {}",
            String::from_utf8_lossy(&gates_run.stderr)
        );
    }
    deployment.pass();
}
