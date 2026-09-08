//! The task declaration: what one browser task asks Weles to do, and the
//! parameter object that asking becomes on the wire.

use serde_json::{json, Map, Value};

mod invocation;
mod receipts;

pub use invocation::submit;
pub use receipts::TaskOutcome;

/// What one browser task asks Weles to do.
pub struct BrowserTask<'a> {
    /// The action name, checked against the host's allowlist.
    pub action: &'a str,
    /// The page the task starts on.
    pub url: &'a str,
    /// What the agent is being asked to accomplish, in words.
    pub objective: &'a str,
    /// Stable recording label. `account_id` controls the browser profile when
    /// a caller explicitly requests a fresh one.
    pub session_label: &'a str,
    /// Exact item passed to a named Weles login trajectory.
    pub login_item: Option<&'a str>,
    /// A unique identity whose SHA-256 names the persistent profile directory.
    pub account_id: Option<&'a str>,
    /// Require Weles to allocate the profile directory atomically.
    pub fresh_profile: bool,
    /// Whether the run's instructions permit signing in.
    ///
    /// This is a HINT, not a gate: Weles appends the constraints to the
    /// model's goal text and enforces none of them — `read_only`, `no_login`
    /// and `no_mutation` appear nowhere else in that product, and the agent
    /// holds fill, click, navigate and store_credential either way. The one
    /// mechanical consequence is here: a vault-backed prefill is refused
    /// unless the caller has said the run may sign in, because handing an
    /// agent credentials while instructing it not to log in is two orders.
    pub allow_login: bool,
    /// Run without a visible window.
    pub headless: bool,
    /// Vault-backed field prefills, each a capability REFERENCE the worker
    /// redeems locally. Empty for a run that carries no sign-in.
    pub credential_prefill: Vec<Value>,
}

impl BrowserTask<'_> {
    /// The parameter object, in the exact shape the `weles-image-inspect`
    /// workload sends for this action, so the two callers of
    /// `generic_browser_task` cannot disagree about its schema.
    ///
    /// `credential_prefill` is added only when there is one, so a run without
    /// a sign-in puts exactly the bytes on the wire it always did.
    pub fn params(&self) -> Value {
        self.params_with(None, &[])
    }

    /// The parameter object for a live submission carrying caller-specific
    /// trajectory identity and capabilities for fields on later pages.
    pub fn params_with(&self, flow_name: Option<&str>, credential_deferred: &[Value]) -> Value {
        let mut constraints = Map::new();
        constraints.insert("read_only".to_string(), json!(!self.allow_login));
        constraints.insert("no_login".to_string(), json!(!self.allow_login));
        constraints.insert("no_mutation".to_string(), json!(!self.allow_login));
        if !self.credential_prefill.is_empty() {
            constraints.insert(
                "credential_prefill".to_string(),
                Value::Array(self.credential_prefill.clone()),
            );
        }
        if !credential_deferred.is_empty() {
            constraints.insert(
                "credential_capabilities".to_string(),
                Value::Array(credential_deferred.to_vec()),
            );
        }
        let mut params = json!({
            "url": self.url,
            "objective": self.objective,
            "flow_name": flow_name.map_or_else(
                || format!("stado-browser-task:{}", self.session_label),
                str::to_string,
            ),
            "session_label": self.session_label,
            "proxy": "none",
            "headless": self.headless,
            "constraints": Value::Object(constraints),
        });
        if let Some(login_item) = self.login_item {
            params["login_item"] = json!(login_item);
        }
        params
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deploy::weles_browser_task::action::DEFAULT_ACTION;

    #[test]
    fn login_is_off_unless_the_caller_asks_for_it() {
        let task = BrowserTask {
            action: DEFAULT_ACTION,
            url: "https://accounts.google.com/",
            objective: "sign in",
            session_label: "oko-calendar",
            allow_login: false,
            headless: true,
            credential_prefill: Vec::new(),
            login_item: None,
            account_id: None,
            fresh_profile: false,
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
        // The schema stays the one the `weles-image-inspect` workload sends.
        assert_eq!(params["proxy"], json!("none"));
        assert!(params["flow_name"]
            .as_str()
            .unwrap()
            .contains("oko-calendar"));
    }

    /// A run without a sign-in must put exactly the bytes on the wire it put
    /// there before this feature existed: no empty `credential_prefill` key
    /// for the trajectory to iterate.
    #[test]
    fn a_run_without_a_sign_in_carries_no_prefill_key_at_all() {
        let task = BrowserTask {
            action: DEFAULT_ACTION,
            url: "https://example.com/",
            objective: "count the images",
            session_label: "plain",
            allow_login: false,
            headless: true,
            credential_prefill: Vec::new(),
            login_item: None,
            account_id: None,
            fresh_profile: false,
        };
        let params = task.params();
        assert!(
            params["constraints"].get("credential_prefill").is_none(),
            "{params}"
        );
        assert_eq!(
            params["constraints"].as_object().unwrap().len(),
            3,
            "{params}"
        );
    }
}
