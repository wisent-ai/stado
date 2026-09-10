//! What the journey's four process results have to say: the native read that
//! must have run for real, and the three refusals that must name the one
//! approved spelling.

use std::process::Output;

use serde_json::{json, Value};

use crate::journey::{write_private, Journey};
use crate::story::TARGET;

pub(crate) fn said(output: &Output) -> String {
    format!(
        "exit={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

pub(crate) fn json_stdout(output: &Output, operation: &str) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "{operation} did not return one JSON document ({error}):\n{}",
            said(output)
        )
    })
}

pub(crate) fn assert_native_read(journey: &Journey, output: &Output) -> (usize, usize) {
    assert!(
        output.status.success(),
        "the real native retained-log read failed; its unmodified process output is retained:\n{}",
        said(output),
    );
    let report = json_stdout(output, "retained-log read");
    assert_eq!(report["schema"], "stado.host-exec-receipt.v1");
    assert_eq!(report["target"], TARGET);
    assert_eq!(report["ssh"], Value::Null);
    assert_eq!(report["ssh_fallbacks"], json!([]));
    assert_eq!(report["command"], journey.story.words.join(" "));
    assert_eq!(report["argv"], json!(journey.story.argv));
    assert_eq!(report["resolved_executable"], journey.story.program);
    assert_eq!(report["status"], "ok");
    assert_eq!(report["exit_code"], 0);
    assert_eq!(report["error"], Value::Null);

    let stdout = report["stdout"]
        .as_str()
        .expect("native stdout is present verbatim in the receipt");
    let stderr = report["stderr"]
        .as_str()
        .expect("native stderr is present verbatim in the receipt");
    write_private(&journey.root.join("native.stdout"), stdout.as_bytes());
    write_private(&journey.root.join("native.stderr"), stderr.as_bytes());
    (stdout.len(), stderr.len())
}

pub(crate) fn assert_refused(output: &Output, words: &[&str], canonical: &str) {
    assert_eq!(
        output.status.code(),
        Some(1),
        "an attempt to widen or modify the native read was not refused with the policy exit:\n{}",
        said(output),
    );
    let report = json_stdout(output, "host-exec refusal");
    let requested = words.join(" ");
    assert_eq!(report["status"], "error");
    assert_eq!(report["failure_point"], "cli.host.exec");
    assert_eq!(report["error_code"], "refused");
    assert_eq!(report["retryable"], false);
    assert_eq!(
        report["message"],
        format!("'{requested}' is not an approved host-exec command"),
    );
    let help = report["help"]
        .as_str()
        .expect("a refusal carries the approved fixed spellings as separate help");
    assert!(
        help.starts_with("approved commands: ") && help.contains(canonical),
        "refusal help omitted the fixed retained-log read: {help}",
    );
}
