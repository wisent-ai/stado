//! On a Linux registry host, the beacon's `link` block reads interface
//! changes from the kernel journal's own fields: every kernel message the
//! journal attributes to the `net` subsystem in the window is one change,
//! whatever its driver writes, and nothing else is.
//!
//! Runs the built `stado host beacon collect` on the host itself (it prints
//! the document and publishes nothing) with the host's declared beacon
//! interval, and reads the kernel journal as JSON: every published change is
//! a `net` message of this boot, and every `net` message of the declared
//! interval is published. Needs a Linux registry host with journalctl and
//! its unit's `WC_HEALTH_INTERVAL_SECONDS` in the environment:
//! `STADO_TEST_LINUX_LINK=1 cargo test --test host-health-linux-link -- --ignored`.

use std::collections::BTreeSet;
use std::process::Command;

use serde_json::Value;

const INTERVAL: &str = "WC_HEALTH_INTERVAL_SECONDS";

/// The flattened `MESSAGE` of every kernel `net` entry `journalctl` prints
/// with `extra` arguments.
fn net_messages(extra: &[&str]) -> BTreeSet<String> {
    let journal = Command::new("journalctl")
        .args(["-k", "--no-pager", "-o", "json"])
        .args(extra)
        .output()
        .expect("journalctl starts");
    assert!(journal.status.success());
    String::from_utf8_lossy(&journal.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|entry| entry["_KERNEL_SUBSYSTEM"] == "net")
        .filter_map(|entry| {
            entry["MESSAGE"]
                .as_str()
                .map(|message| message.split_whitespace().collect::<Vec<_>>().join(" "))
        })
        .collect()
}

#[test]
#[ignore = "reads a Linux registry host's own kernel journal"]
fn interface_changes_are_the_kernels_net_messages_of_the_window() {
    if std::env::var("STADO_TEST_LINUX_LINK").as_deref() != Ok("1") {
        panic!("set STADO_TEST_LINUX_LINK=1 on a Linux registry host");
    }
    let seconds = std::env::var(INTERVAL)
        .expect("WC_HEALTH_INTERVAL_SECONDS carries the host's declared beacon interval");
    let collected = Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["host", "beacon", "collect"])
        .output()
        .expect("the built stado starts");
    assert!(
        collected.status.success(),
        "{}",
        String::from_utf8_lossy(&collected.stderr)
    );
    let document: Value =
        serde_json::from_slice(&collected.stdout).expect("the beacon document is JSON");
    let link = &document["link"];
    assert_eq!(link["source"], "linux", "{link}");
    let published: BTreeSet<String> = link["interface_changes"]
        .as_array()
        .expect("the link block lists its interface changes")
        .iter()
        .filter_map(|change| change["detail"].as_str().map(str::to_string))
        .collect();

    let this_boot = net_messages(&["--boot"]);
    assert!(published.is_subset(&this_boot), "{published:?}");
    let interval = net_messages(&["--since", &format!("-{seconds}s")]);
    assert!(interval.is_subset(&published), "{interval:?} {published:?}");
}
