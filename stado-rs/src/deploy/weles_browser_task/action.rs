//! The step vocabulary: the action names one host's worker will accept, read
//! off that host, and the refusal for a name that is not among them.

use std::collections::BTreeSet;

use crate::deploy::{service_file_fetch, DeployError, Runner};
use crate::targets::ComputeTarget;

/// The env key whose value is the comma-separated list of actions a worker
/// will accept.
pub const ALLOWLIST_KEY: &str = "WELES_ACTION_ALLOWLIST";

/// The action a general browser task runs as. Not a fixed constant the way
/// [`crate::deploy::weles_capture::CAPTURE_ACTION`] is — it is this module's
/// default, and the caller may name another, but either way the host's
/// allowlist decides.
pub const DEFAULT_ACTION: &str = "generic_browser_task";

/// The immutable action catalog shipped by the active Weles release.
pub const DEFAULT_ALLOWLIST_FILE: &str = "$HOME/weles/src/worker/deploy/weles-action-allowlist.txt";

/// Every action one host will accept, in the order the file lists them.
///
/// The canonical file is one action per line. A legacy worker env assignment
/// remains readable so an older active release can still explain its own gate.
pub fn parse_allowlist(body: &str) -> Vec<String> {
    let mut found: Option<&str> = None;
    for line in body.lines() {
        let trimmed = line.trim_start();
        let assignment = trimmed
            .strip_prefix("export ")
            .map_or(trimmed, str::trim_start);
        if let Some(value) = assignment.strip_prefix(&format!("{ALLOWLIST_KEY}=")) {
            found = Some(value);
        }
    }
    let legacy = found.is_some();
    let content = found
        .map(crate::deploy::service_env_file::effective_text)
        .unwrap_or(body);
    let entries: Vec<&str> = if legacy {
        content.split(',').collect()
    } else {
        content.lines().collect()
    };
    let mut seen = BTreeSet::new();
    let mut actions = Vec::with_capacity(entries.len());
    for entry in entries {
        let action = entry.trim();
        if action.is_empty() {
            continue;
        }
        if !action
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
            || !seen.insert(action)
        {
            return Vec::new();
        }
        actions.push(action.to_string());
    }
    actions
}

/// Read one host's action allowlist byte-exactly.
pub async fn host_allowlist(
    target: &ComputeTarget,
    allowlist_file: &str,
    runner: &Runner,
) -> Result<Vec<String>, DeployError> {
    let fetched = service_file_fetch::fetch_file(target, allowlist_file, runner).await?;
    if !fetched.ok() {
        return Err(DeployError(format!(
            "{}: could not read {allowlist_file} to learn which actions this worker accepts: {} ({})",
            target.name,
            fetched.report.file_state,
            if fetched.report.detail.is_empty() {
                fetched.integrity
            } else {
                &fetched.report.detail
            }
        )));
    }
    let body = String::from_utf8_lossy(&fetched.content).into_owned();
    Ok(parse_allowlist(&body))
}

/// Refuse an action this host's worker would refuse, naming both.
///
/// The sentence lists what the host does accept for the shape asked for, so an
/// operator who named `generic_capture` is told which generic action exists
/// instead of being left to read a 226-entry list.
pub fn ensure_allowed(host: &str, action: &str, allowlist: &[String]) -> Result<(), DeployError> {
    if allowlist.iter().any(|entry| entry == action) {
        return Ok(());
    }
    if allowlist.is_empty() {
        return Err(DeployError(format!(
            "{host} declares no {ALLOWLIST_KEY}, so no action can be shown to be accepted there; \
             the worker refuses every name outside that list"
        )));
    }
    let generic: Vec<&str> = allowlist
        .iter()
        .filter(|entry| entry.starts_with("generic_"))
        .map(String::as_str)
        .collect();
    let mut said = format!(
        "{host} does not accept the action {action:?}: its {ALLOWLIST_KEY} carries {} action(s) \
         and that is not one of them, so the worker would refuse the job",
        allowlist.len()
    );
    if !generic.is_empty() {
        said.push_str(&format!(
            ". The general action(s) it does accept: {}",
            generic.join(", ")
        ));
    }
    Err(DeployError(said))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_allowlist_is_the_last_assignment_and_survives_quotes() {
        // A sourced file assigns top to bottom, so a later duplicate wins.
        let body = "WELES_ACTION_ALLOWLIST=apple_login,discord_login\n\
                    OTHER=\
                    1\n\
                    export WELES_ACTION_ALLOWLIST='generic_browser_task, google_search ,apple_login'\n";
        assert_eq!(
            parse_allowlist(body),
            vec!["generic_browser_task", "google_search", "apple_login"]
        );
    }

    #[test]
    fn a_file_without_the_key_yields_no_actions_rather_than_a_guess() {
        assert!(parse_allowlist("WELES_HEADLESS=1\n").is_empty());
    }

    #[test]
    fn an_action_the_host_does_not_carry_is_refused_naming_action_and_host() {
        // The exact case: `generic_capture` is what the `weles-capture`
        // workload declares, and charless-mac-mini's worker does not accept it.
        let allow = vec![
            "generic_browser_task".to_string(),
            "generic_saved_task".to_string(),
            "apple_login".to_string(),
        ];
        let error = ensure_allowed("charless-mac-mini", "generic_capture", &allow).unwrap_err();
        let said = error.to_string();
        assert!(said.contains("generic_capture"), "{said}");
        assert!(said.contains("charless-mac-mini"), "{said}");
        assert!(said.contains("3 action(s)"), "{said}");
        // It names the general action that does exist, so the operator is not
        // left reading a 226-entry list.
        assert!(said.contains("generic_browser_task"), "{said}");
        assert!(said.contains("generic_saved_task"), "{said}");
    }

    #[test]
    fn an_allowed_action_passes() {
        let allow = vec!["generic_browser_task".to_string()];
        assert!(ensure_allowed("h", "generic_browser_task", &allow).is_ok());
    }

    #[test]
    fn an_absent_allowlist_is_refused_rather_than_treated_as_permissive() {
        let error = ensure_allowed("h", "generic_browser_task", &[]).unwrap_err();
        assert!(error
            .to_string()
            .contains("declares no WELES_ACTION_ALLOWLIST"));
    }
}
