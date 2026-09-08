use serde_json::Value;

use crate::cli::host::checks::probes::cell;

/// The answer half of [`super::inventory`]: the reconciliation, declaration
/// and version axes, and the two vault findings.
pub(super) fn print_reconciliation(report: &Value, vaults: &[Value], sidecars: &[Value]) {
    // The answer, not the raw tables above it.
    let summary = report.get("reconciliation");
    let counted = |key: &str| cell(summary.and_then(|value| value.get(key)));
    println!(
        "\nreconciliation: {} of {} forward markers matched, {} stale, {} unreadable, \
         {} unjudged",
        counted("matched"),
        counted("markers"),
        counted("stale"),
        counted("unreadable"),
        counted("unknown"),
    );
    let name_list = |key: &str| -> Vec<String> {
        summary
            .and_then(|value| value.get(key))
            .and_then(Value::as_array)
            .map(|names| names.iter().map(|name| cell(Some(name))).collect())
            .unwrap_or_default()
    };
    let stale = name_list("stale_markers");
    if !stale.is_empty() {
        println!(
            "stale markers:  {} — the marker names a port nothing is listening on",
            stale.join(", ")
        );
    }

    // The registry axis, said in words. It is deliberately not folded into
    // the line above: "matched" there means something is listening, and a
    // marker can be listening and still send consumers to a port the
    // directory does not declare — which is the drift that survives every
    // health check.
    println!(
        "declaration:    {} of {} forward markers agree with the registry, {} disagree, \
         {} undeclared",
        counted("declaration_matched"),
        counted("markers"),
        counted("declaration_disagrees"),
        counted("declaration_undeclared"),
    );
    let disagreeing = name_list("disagreeing_markers");
    let undeclared_markers = name_list("undeclared_markers");
    if !disagreeing.is_empty() {
        println!(
            "marker vs registry: {} — the marker points at one endpoint and the \
             service directory declares another for this host; consumers resolving \
             through the directory do not arrive where the marker says",
            disagreeing.join(", ")
        );
    }
    if !undeclared_markers.is_empty() {
        println!(
            "undeclared markers: {} — no service in the directory carries an endpoint \
             for this host under that name, so there is nothing to hold the marker to",
            undeclared_markers.join(", ")
        );
    }

    // The version axis. `undeclared` is printed too: a host nobody declared
    // a version for is not a host that passed a version check.
    let behind = name_list("versions_behind");
    let ahead = name_list("versions_ahead");
    let mismatched = name_list("versions_mismatched");
    let unjudged = name_list("versions_unjudged");
    let undeclared_versions = name_list("versions_undeclared");
    if !behind.is_empty() {
        println!(
            "versions behind: {} — older than registry managed_versions declares \
             for this host",
            behind.join(", ")
        );
    }
    if !ahead.is_empty() {
        println!(
            "versions ahead:  {} — newer than registry managed_versions declares; \
             the declaration is the thing that is stale",
            ahead.join(", ")
        );
    }
    if !mismatched.is_empty() {
        println!(
            "versions differ: {} — installed and declared are not the same string, \
             and one of them is not three numbers, so neither is older",
            mismatched.join(", ")
        );
    }
    if !unjudged.is_empty() {
        println!(
            "versions unjudged: {} — the registry declares a version and the host \
             reported none that could be read",
            unjudged.join(", ")
        );
    }
    if !undeclared_versions.is_empty() {
        println!(
            "versions undeclared: {} — the registry declares no required version for \
             this host, so nothing here was verified against a target state",
            undeclared_versions.join(", ")
        );
    }
    if disagreeing.is_empty()
        && undeclared_markers.is_empty()
        && behind.is_empty()
        && ahead.is_empty()
        && mismatched.is_empty()
        && unjudged.is_empty()
        && undeclared_versions.is_empty()
    {
        // One confirming line, for the same reason the vault section has
        // one: a verified host must read as verified, not as a section that
        // printed nothing.
        println!(
            "declared state: every marker matches the endpoint the registry declares, \
             and every managed binary is at its declared version"
        );
    }
    // The two vault findings, in the human output and not only under --json:
    // a signal an operator has to ask for in JSON is a signal they will miss.
    let not_owner_only = name_list("vaults_not_owner_only");
    let refused = name_list("vaults_refused");
    if !not_owner_only.is_empty() {
        println!(
            "vault perms:    {} — readable past the owner; a vault the group \
             can read is an incident, not cosmetics",
            not_owner_only.join(", ")
        );
    }
    if !refused.is_empty() {
        println!(
            "vaults refused: {} — a symlink or not a regular file, reported \
             rather than followed",
            refused.join(", ")
        );
    }
    if not_owner_only.is_empty() && refused.is_empty() {
        // Say it, rather than printing nothing: a clean host must read as
        // checked, not as a section that quietly had nothing to add.
        println!(
            "vaults:         {} active, {} sidecar — all owner-only, none refused",
            vaults.len(),
            sidecars.len()
        );
    }
}
