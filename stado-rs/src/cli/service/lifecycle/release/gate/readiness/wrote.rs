//! What the unit wrote while the gate waited for it to answer.
//!
//! A release that never answers its readiness URL is rolled back with
//! `readiness timed out after <n>s: <url> never answered`, and that sentence
//! is the whole record the run keeps: the unit's own account of why it did
//! not bind sits in the log launchd or the journal gave it, on the host, and
//! nothing in the run names a line of it. The gate now reads that log whole
//! before the restart and again after the failure, and the lines the second
//! read carries beyond the first are what the unit wrote in the window. No
//! count is chosen: the window's output is measured as the difference of the
//! two reads, and a log that was rotated or truncated between them is
//! reported whole, as the second read found it, because no prefix of it is
//! known to predate the restart.

use crate::deploy::service::{self, ManagedService, ServiceLog};
use crate::deploy::Runner;
use crate::targets::ComputeTarget;

/// The unit's logs as they stand, or the sentence that explains why they
/// could not be read. A log that cannot be read does not stop a release:
/// it is one piece of evidence, and its absence is said in the failure
/// instead of standing in for the failure.
pub(in super::super) async fn snapshot(
    target: &ComputeTarget,
    unit: &ManagedService,
    runner: &Runner,
) -> Result<ServiceLog, String> {
    service::tail_unit_logs(target, unit.unit_id(), &unit.path, None, runner)
        .await
        .map_err(|error| format!("{error}"))
}

/// The lines `after` carries beyond `before` for one stream, or the whole of
/// `after` when `before` is not its prefix.
fn appended<'a>(before: &str, after: &'a str) -> &'a str {
    match after.strip_prefix(before) {
        Some(written) => written,
        None => after,
    }
}

/// One sentence per stream naming what the unit wrote in the window, from
/// the two snapshots, or the read failure that stands in for a snapshot.
pub(in super::super) fn during_window(
    unit: &str,
    before: &Result<ServiceLog, String>,
    after: &Result<ServiceLog, String>,
) -> String {
    let (before, after) = match (before, after) {
        (Ok(before), Ok(after)) => (before, after),
        (Err(refusal), _) => {
            return format!(
                "what {unit} wrote in the readiness window is unknown: its log could not be \
                 read before the restart: {refusal}"
            )
        }
        (_, Err(refusal)) => {
            return format!(
                "what {unit} wrote in the readiness window is unknown: its log could not be \
                 read after the readiness failure: {refusal}"
            )
        }
    };
    let mut account = Vec::new();
    let out = appended(&before.body, &after.body);
    account.push(if out.trim().is_empty() {
        format!(
            "{unit} wrote nothing to {} in the readiness window",
            after.origin
        )
    } else {
        format!(
            "{unit} wrote to {} in the readiness window:\n{}",
            after.origin,
            out.trim_end()
        )
    });
    // stderr is its own file under launchd; when the unit sends both streams
    // to one file the stdout read already carries it, so it is not repeated.
    if let Some(origin) = &after.error_origin {
        let err = appended(&before.error_body, &after.error_body);
        if !err.trim().is_empty() && *origin != after.origin {
            account.push(format!(
                "{unit} wrote to {origin} in the readiness window:\n{}",
                err.trim_end()
            ));
        }
    }
    account.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(body: &str, error_origin: Option<&str>, error_body: &str) -> ServiceLog {
        ServiceLog {
            host: "host".into(),
            unit: "com.example.unit".into(),
            origin: "/var/log/unit.log".into(),
            body: body.into(),
            error_origin: error_origin.map(str::to_owned),
            error_body: error_body.into(),
        }
    }

    #[test]
    fn the_lines_beyond_the_first_read_are_the_window() {
        let before = Ok(log("started\n", Some("/var/log/unit.err"), ""));
        let after = Ok(log(
            "started\nbinding\n",
            Some("/var/log/unit.err"),
            "refused: no socket\n",
        ));
        let account = during_window("com.example.unit", &before, &after);
        assert!(account.contains("wrote to /var/log/unit.log in the readiness window:\nbinding"));
        assert!(account
            .contains("wrote to /var/log/unit.err in the readiness window:\nrefused: no socket"));
        assert!(!account.contains("started"));
    }

    #[test]
    fn a_truncated_log_is_reported_whole() {
        let before = Ok(log("old lines that were rotated away\n", None, ""));
        let after = Ok(log("fresh\n", None, ""));
        let account = during_window("com.example.unit", &before, &after);
        assert!(account.ends_with("in the readiness window:\nfresh"));
    }

    #[test]
    fn nothing_written_is_said_and_a_shared_file_is_not_repeated() {
        let before = Ok(log("a\n", Some("/var/log/unit.log"), "a\n"));
        let after = Ok(log("a\n", Some("/var/log/unit.log"), "a\n"));
        let account = during_window("com.example.unit", &before, &after);
        assert_eq!(
            account,
            "com.example.unit wrote nothing to /var/log/unit.log in the readiness window"
        );
    }

    #[test]
    fn an_unreadable_log_names_which_read_failed() {
        let after = Ok(log("", None, ""));
        let account = during_window("com.example.unit", &Err("ssh refused".into()), &after);
        assert!(account.contains("could not be read before the restart: ssh refused"));
    }
}
