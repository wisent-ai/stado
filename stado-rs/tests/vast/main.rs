//! The Vast.ai earning bridge, driven as the real binary on a machine that
//! holds no credential — the state every host in this fleet was in on
//! 2026-09-20, when `stado vast status` answered `Skarbiec item stado-vast
//! field api_key is required` under a `403 consumer not authorized to read
//! item field`, and the vault that would hold the item declared none.
//!
//! Three stories: a dry run decides and prints without any credential, a
//! live run refuses and names the channel that asked, and `readiness` states
//! a verdict with the commands that close the gap.

mod support;

use serde_json::Value;

use support::Bridge;

/// The idle window a story uses so the first poll already decides.
const IMMEDIATE: &str = "0";
const ONE_SECOND: &str = "1";
/// The line the preview prints once it has decided to list: the poll after
/// the decision reports the state it would have written.
const LISTED_STATE: &str = "(listed=True)";

#[test]
fn a_dry_run_decides_without_any_credential() {
    let bridge = Bridge::new();
    let (stdout, stderr, alive) = bridge.observe_daemon(
        &[
            "vast",
            "auto-list",
            "--dry-run",
            "--idle-window-s",
            IMMEDIATE,
            "--poll-interval-s",
            ONE_SECOND,
        ],
        LISTED_STATE,
    );
    assert!(
        alive,
        "the dry run exited instead of looping:\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("startup: no Vast.ai credential; printing decisions only"),
        "the dry run hid that it holds no credential:\n{stdout}"
    );
    assert_eq!(
        stdout.matches("DRY-RUN would list").count(),
        1,
        "a preview that decides to list again every poll is not a preview:\n{stdout}"
    );
    assert!(
        stdout.contains(LISTED_STATE),
        "after deciding to list, the preview must carry that state:\n{stdout}"
    );
    assert!(
        !stdout.contains("LISTED ("),
        "a dry run must not list on the marketplace:\n{stdout}"
    );
}

#[test]
fn a_live_run_refuses_and_names_the_channel_that_asked() {
    let bridge = Bridge::new();
    let output = bridge.invoke(&["vast", "auto-list", "--idle-window-s", IMMEDIATE]);
    assert_eq!(output.status.code(), Some(1), "a live run must refuse");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no Vast.ai API key on this host"),
        "the refusal did not name the missing key:\n{stderr}"
    );
    assert!(
        stderr.contains("no Skarbiec channel on this host"),
        "the refusal did not name the channel state:\n{stderr}"
    );
    assert!(
        stderr.contains("stado vast readiness"),
        "the refusal did not name the command that diagnoses it:\n{stderr}"
    );
}

#[test]
fn readiness_states_the_verdict_and_the_way_out() {
    let bridge = Bridge::new();
    let output = bridge.invoke(&["vast", "readiness", "--no-vault-check", "--json"]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "a machine that cannot earn must exit non-zero"
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "readiness printed no document: {error}\n{}",
            String::from_utf8_lossy(&output.stdout)
        )
    });
    assert_eq!(report["document"], "stado.vast-readiness.v1", "{report}");
    assert_eq!(report["verdict"], "no_channel", "{report}");
    assert_eq!(report["item"], "stado-vast", "{report}");
    assert_eq!(report["field"], "api_key", "{report}");
    assert_eq!(report["channel"]["kind"], "none", "{report}");
    let remedy = report["remedy"]
        .as_array()
        .unwrap_or_else(|| panic!("readiness names what to do: {report}"));
    assert!(!remedy.is_empty(), "a refusal with no way out: {report}");
}

#[test]
fn readiness_in_text_names_the_item_and_the_channel() {
    let bridge = Bridge::new();
    let output = bridge.invoke(&["vast", "readiness", "--no-vault-check"]);
    assert_eq!(output.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("item:     stado-vast/api_key"),
        "the report did not name the credential it needs:\n{stdout}"
    );
    assert!(
        stdout.contains("no Skarbiec channel on this host"),
        "the report did not name the channel state:\n{stdout}"
    );
}
