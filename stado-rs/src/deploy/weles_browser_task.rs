//! Submit one browser task to Weles on a target host, with the action name
//! taken from that host's own allowlist.
//!
//! NO Python original. This module exists because of a gap found on
//! 2026-08-30 while trying to drive a sign-in through Weles on
//! charless-mac-mini. Stado had exactly two ways to put work on a Weles
//! worker and neither could carry an operator's task:
//!
//! - `host weles-capture` hard-codes `generic_capture`
//!   ([`super::weles_capture::CAPTURE_ACTION`]), and that action is not in
//!   that host's 226-entry `WELES_ACTION_ALLOWLIST`. The worker refuses any
//!   name outside the allowlist, so the command cannot run there at all.
//! - `host weles-image-inspect` does submit the allowlisted
//!   `generic_browser_task`, but its objective and constraints are fixed in
//!   product code: read-only, no login, no mutation, and an objective about
//!   counting rendered images.
//!
//! So the one action the host would accept was reachable only through a
//! command that could not be told what to do. Every browser workflow this
//! fleet is supposed to own sat behind that.
//!
//! Two properties are deliberate:
//!
//! 1. **The action comes from the host, not from a constant.** The allowlist
//!    is read off the target and the requested action is checked against it
//!    BEFORE any channel is opened, so a name the worker would refuse is
//!    refused here with a sentence naming the action and the host — rather
//!    than enqueued, accepted, and silently dropped. `generic_capture` is
//!    exactly that case and is why this rule exists.
//! 2. **The allowlist is read byte-exact.** `service env-show` clamps every
//!    reported value at 400 characters, and that allowlist is 4488 — reading
//!    it through the diagnostic reader would silently truncate the list to its
//!    first 25 entries and refuse 200 legitimate actions. It is read through
//!    [`super::service_file_fetch`], whose whole contract is that the bytes
//!    arrive unaltered, and the file is never written to disk or printed.

use serde_json::{json, Map, Value};

use super::{service_file_fetch, weles_capture, DeployError, Runner};
use crate::targets::ComputeTarget;

/// The env key whose value is the comma-separated list of actions a worker
/// will accept.
pub const ALLOWLIST_KEY: &str = "WELES_ACTION_ALLOWLIST";

/// The action a general browser task runs as. Not a fixed constant the way
/// [`super::weles_capture::CAPTURE_ACTION`] is — it is this module's default,
/// and the caller may name another, but either way the host's allowlist
/// decides.
pub const DEFAULT_ACTION: &str = "generic_browser_task";

/// The env file a Weles worker sources on this fleet.
pub const DEFAULT_ENV_FILE: &str = "$HOME/.config/weles/worker.env";

/// Every action one host will accept, in the order the file lists them.
///
/// Parsed from the LAST assignment of [`ALLOWLIST_KEY`], because a sourced
/// file assigns top to bottom and a later duplicate silently wins — the same
/// rule [`super::service_env_file::shadowing`] reports on.
pub fn parse_allowlist(env_body: &str) -> Vec<String> {
    let mut found: Option<&str> = None;
    for line in env_body.lines() {
        let trimmed = line.trim_start();
        let assignment = trimmed
            .strip_prefix("export ")
            .map_or(trimmed, str::trim_start);
        if let Some(value) = assignment.strip_prefix(&format!("{ALLOWLIST_KEY}=")) {
            found = Some(value);
        }
    }
    let Some(raw) = found else { return Vec::new() };
    let unquoted = super::service_env_file::effective_text(raw);
    unquoted
        .split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect()
}

/// Read one host's action allowlist.
///
/// The env file is fetched byte-exact and kept in memory only: it carries the
/// worker's credentials and this function wants one line of it.
pub async fn host_allowlist(
    target: &ComputeTarget,
    env_file: &str,
    runner: &Runner,
) -> Result<Vec<String>, DeployError> {
    let fetched = service_file_fetch::fetch_file(target, env_file, runner).await?;
    if !fetched.ok() {
        return Err(DeployError(format!(
            "{}: could not read {env_file} to learn which actions this worker accepts: {} ({})",
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

/// What one browser task asks Weles to do.
pub struct BrowserTask<'a> {
    /// The action name, checked against the host's allowlist.
    pub action: &'a str,
    /// The page the task starts on.
    pub url: &'a str,
    /// What the agent is being asked to accomplish, in words.
    pub objective: &'a str,
    /// A stable label for the browser session, so a resumed flow reuses one
    /// profile instead of starting anonymous every time.
    pub session_label: &'a str,
    /// Whether the run may sign in. `false` sends the same read-only,
    /// no-login, no-mutation constraints `host weles-image-inspect` fixes;
    /// `true` is for the flows whose whole purpose is authentication, and it
    /// is the caller's explicit decision rather than a default.
    pub allow_login: bool,
    /// Run without a visible window.
    pub headless: bool,
}

impl BrowserTask<'_> {
    /// The parameter object, in the exact shape
    /// `host weles-image-inspect` already sends for this action — so the two
    /// callers of `generic_browser_task` cannot disagree about its schema.
    pub fn params(&self) -> Value {
        json!({
            "url": self.url,
            "objective": self.objective,
            "flow_name": format!("stado-browser-task:{}", self.session_label),
            "session_label": self.session_label,
            "proxy": "none",
            "headless": self.headless,
            "constraints": {
                "read_only": !self.allow_login,
                "no_login": !self.allow_login,
                "no_mutation": !self.allow_login,
            },
        })
    }
}

/// Everything one completed task reports back.
pub struct TaskOutcome {
    pub run_id: String,
    pub ok: bool,
    pub exit_code: Option<i64>,
    pub result: Value,
}

impl TaskOutcome {
    pub fn to_report(&self, target: &str, action: &str) -> Map<String, Value> {
        let mut object = Map::new();
        object.insert("host".to_string(), json!(target));
        object.insert("action".to_string(), json!(action));
        object.insert("run_id".to_string(), json!(self.run_id));
        object.insert("ok".to_string(), json!(self.ok));
        object.insert("exit_code".to_string(), json!(self.exit_code));
        object.insert("result".to_string(), self.result.clone());
        object
    }
}

/// Submit the task and carry it through to its result.
///
/// Synchronous by construction: [`weles_capture::observe_action_payload`]
/// holds the request open for the run and returns what the run produced, so
/// this command reports an outcome rather than a queue receipt. A caller that
/// only wanted a receipt would have to poll the action log, and a browser
/// flow whose result nobody read is how a sign-in silently fails.
pub async fn submit(target: &str, task: &BrowserTask<'_>) -> Result<TaskOutcome, DeployError> {
    let admission = weles_capture::resolve_admission(target).await?;
    let channel = weles_capture::open_channel(&admission).await?;
    let payload =
        weles_capture::observe_action_payload(&channel, task.action, task.params()).await?;
    let run_id = payload
        .get("run_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Ok(TaskOutcome {
        ok: payload.get("ok").and_then(Value::as_bool).unwrap_or(false),
        exit_code: payload.get("exitCode").and_then(Value::as_i64),
        result: payload.get("result").cloned().unwrap_or(Value::Null),
        run_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_allowlist_is_the_last_assignment_and_survives_quotes() {
        // A sourced file assigns top to bottom, so a later duplicate wins.
        let body = "WELES_ACTION_ALLOWLIST=apple_login,discord_login\n\
                    OTHER=1\n\
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
        // The exact case: `generic_capture` is what `host weles-capture`
        // hard-codes, and charless-mac-mini's worker does not accept it.
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

    #[test]
    fn login_is_off_unless_the_caller_asks_for_it() {
        let task = BrowserTask {
            action: DEFAULT_ACTION,
            url: "https://accounts.google.com/",
            objective: "sign in",
            session_label: "oko-calendar",
            allow_login: false,
            headless: true,
        };
        let params = task.params();
        assert_eq!(params["constraints"]["no_login"], json!(true));
        assert_eq!(params["constraints"]["read_only"], json!(true));

        let permitted = BrowserTask {
            allow_login: true,
            ..task
        };
        let params = permitted.params();
        assert_eq!(params["constraints"]["no_login"], json!(false));
        assert_eq!(params["constraints"]["no_mutation"], json!(false));
        // The schema stays the one `weles-image-inspect` already sends.
        assert_eq!(params["proxy"], json!("none"));
        assert!(params["flow_name"]
            .as_str()
            .unwrap()
            .contains("oko-calendar"));
    }
}
