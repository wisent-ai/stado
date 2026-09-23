//! `stado host cleanup TARGET --dry-run` — preview what the registry
//! cleanup would delete on one host, deleting nothing.
//!
//! NO Python original: item five of `docs/missing-commands.md`. Shape and
//! rules come from [`crate::deploy::host_reboot`] via
//! [`crate::deploy::host_channel`].
//!
//! This module contains NO cleanup policy. It cannot: the policy is a
//! bounded, dir_fd-relative walk of the target's own filesystem
//! ([`crate::providers::local::disk_cleanup`]), and the only machine that
//! can see that filesystem is the target. A second, remote re-implementation
//! of "what would be deleted" would be a second answer to the question, and
//! the first time the two disagreed the operator would believe the wrong
//! one.
//!
//! So the preview runs where the files are. The remote program locates the
//! host's own stado binary through
//! [`crate::deploy::host_recovery::WC_CANDIDATES`] — the same discovery
//! list `host recover` uses to run the real cleanup — and invokes
//! `disk-cleanup --once --dry-run`, which is
//! [`crate::providers::local::disk_cleanup::preview_cleanup_once`]: the
//! janitor's own planning phase with an `enforce` policy pinned down to its
//! own `report` mode and no state written. What comes back is the janitor's
//! canonical report, parsed but not reinterpreted.
//!
//! A host whose stado predates `--dry-run` reports `unavailable` with the
//! remote's own message rather than falling back to anything destructive.
//!
//! Like [`crate::deploy::host_recovery`]'s script, the remote program is
//! written as an escaped string: `\\t` / `\\n` are the literal backslash
//! sequences the remote `printf` expands.

use serde_json::{json, Map, Value};

use super::host_channel;
use super::host_recovery::WC_CANDIDATES;
use super::{DeployError, Runner};
use crate::targets::ComputeTarget;

/// `status` for a preview that ran and produced a plan.
pub const PREVIEW_STATUS: &str = "preview";
/// `status` for a host that has no stado to preview with, or whose stado
/// does not understand `--dry-run`.
pub const UNAVAILABLE_STATUS: &str = "unavailable";

/// Substitution point for the binary-discovery list in
/// [`REMOTE_SCRIPT_TEMPLATE`].
const WC_WORDS_MARK: &str = "@WC_WORDS@";

/// The fixed remote program.
///
/// stderr is deliberately NOT redirected: it travels back over ssh into the
/// channel's own stderr, which is where [`host_channel::finish_report`]
/// reads the last line from. Redirecting it to `/dev/null` — as the
/// recovery script does, because it has a report to protect — would throw
/// away the one sentence explaining why a preview failed.
const REMOTE_SCRIPT_TEMPLATE: &str = "set -u
wc_bin=\"\"
for candidate in @WC_WORDS@; do
  if [ -x \"$candidate\" ]; then wc_bin=\"$candidate\"; break; fi
done
if [ -z \"$wc_bin\" ]; then
  printf 'STADO_PREVIEW\\tunavailable\\t%s\\n' 'no stado binary on this host'
  exit 66
fi
printf 'STADO_PREVIEW_BIN\\t%s\\n' \"$wc_bin\"
plan=$(\"$wc_bin\" disk-cleanup --once --dry-run)
plan_rc=$?
if [ \"$plan_rc\" -ne 0 ]; then
  printf 'STADO_PREVIEW\\tunavailable\\t%s\\n' \"disk-cleanup --dry-run exited $plan_rc\"
  exit \"$plan_rc\"
fi
printf 'STADO_PREVIEW\\tok\\t%s\\n' \"$plan\"
";

/// The remote program with the binary-discovery list in place.
///
/// The candidates are quoted exactly the way
/// [`crate::deploy::host_recovery::remote_script`] quotes them, so `$HOME`
/// still expands on the remote side while the word stays one word.
pub fn remote_script() -> String {
    let wc_words = WC_CANDIDATES
        .iter()
        .map(|value| format!("\"{value}\""))
        .collect::<Vec<String>>()
        .join(" ");
    REMOTE_SCRIPT_TEMPLATE.replace(WC_WORDS_MARK, &wc_words)
}

/// What the remote preview reported.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PreviewOutcome {
    /// The janitor's canonical report, verbatim.
    pub plan: Option<Value>,
    /// The stado binary the host used.
    pub binary: Option<String>,
    /// Why no plan came back.
    pub unavailable: Option<String>,
}

/// Fold the marker lines of stdout into an outcome.
pub fn parse_output(stdout: &str) -> PreviewOutcome {
    let mut outcome = PreviewOutcome::default();
    for line in stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_PREVIEW_BIN", path] => outcome.binary = Some((*path).to_string()),
            ["STADO_PREVIEW", "ok", payload] => match serde_json::from_str(payload) {
                Ok(plan) => outcome.plan = Some(plan),
                // The janitor prints one line of canonical JSON. Anything
                // else means the remote binary is not the one we think it
                // is, which is worth saying out loud rather than papering
                // over with an empty plan.
                Err(exc) => outcome.unavailable = Some(exc.to_string()),
            },
            ["STADO_PREVIEW", "unavailable", detail] => {
                outcome.unavailable = Some((*detail).to_string());
            }
            _ => {}
        }
    }
    outcome
}

/// Per-cleaner summary of a plan, pulled straight out of the janitor's
/// report so the numbers are the janitor's own.
///
/// `eligible_items` and `expected_bytes` are what a real pass would remove;
/// `deleted_items` is present for completeness and MUST be zero in a
/// preview, which is exactly why it is worth showing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CleanerPlan {
    pub name: String,
    pub scanned_items: i64,
    pub eligible_items: i64,
    pub deleted_items: i64,
    pub expected_bytes: i64,
}

/// The per-cleaner rows of a janitor report, in the report's own key order.
pub fn cleaner_plans(plan: &Value) -> Vec<CleanerPlan> {
    let Some(cleaners) = plan.get("cleaners").and_then(Value::as_object) else {
        return Vec::new();
    };
    let count =
        |section: &Value, key: &str| section.get(key).and_then(Value::as_i64).unwrap_or_default();
    cleaners
        .iter()
        .map(|(name, section)| CleanerPlan {
            name: name.clone(),
            scanned_items: count(section, "scanned_items"),
            eligible_items: count(section, "eligible_items"),
            deleted_items: count(section, "deleted_items"),
            expected_bytes: count(section, "expected_bytes"),
        })
        .collect()
}

/// The outcome as the `--json` report, in `host reboot`'s report shape.
pub fn to_report(target: &ComputeTarget, outcome: &PreviewOutcome) -> Map<String, Value> {
    let mut report = host_channel::base_report(target);
    report.insert("stado_binary".to_string(), json!(outcome.binary));
    report.insert(
        "plan".to_string(),
        outcome.plan.clone().unwrap_or(Value::Null),
    );
    report.insert("unavailable".to_string(), json!(outcome.unavailable));
    // The mode the REGISTRY declares, next to the plan the preview
    // produced. The plan's own `mode` is the previewed mode, so without
    // this an operator could not tell an `enforce` policy previewed
    // read-only from a policy that is genuinely switched off.
    report.insert(
        "registry_policy_mode".to_string(),
        target
            .disk_cleanup
            .as_ref()
            .map_or(Value::Null, |policy| json!(policy.mode)),
    );
    report
}

/// Preview the registry cleanup on one canonical registry host.
pub async fn cleanup_preview(target_name: &str, runner: &Runner) -> Result<Value, DeployError> {
    let target = host_channel::canonical_target(target_name).await?;
    let output = host_channel::run_script(&target, &remote_script(), runner).await?;
    let outcome = parse_output(&output.stdout);
    let mut report = to_report(&target, &outcome);
    host_channel::finish_report(&mut report, &output, PREVIEW_STATUS, "ssh failed");
    // The remote distinguishes "I could not preview" from "ssh broke", and
    // that distinction is worth keeping: a host with no stado on it is a
    // provisioning gap, not an unreachable box. finish_report only sees an
    // exit code, so the explicit marker overrides it — including its
    // `error`, because the remote's own sentence beats the last line of a
    // stream that is mostly marker protocol.
    if let Some(detail) = &outcome.unavailable {
        report.insert("status".to_string(), json!(UNAVAILABLE_STATUS));
        report.insert("error".to_string(), json!(detail));
    }
    Ok(Value::Object(report))
}
