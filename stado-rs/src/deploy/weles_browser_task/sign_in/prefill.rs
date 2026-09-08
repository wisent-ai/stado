//! The capability references a sign-in run carries: the entry shape the
//! trajectory destructures, and the issuance that mints one pair on the
//! redeeming host.

use serde_json::{json, Value};

use super::routes::{confirm_routed_item, fill_resource};
use super::scopes::{host_scopes, scope_consumer};
use super::{
    weles_api_broker_files, CAPABILITY_TARGET, FILL_PURPOSE, SIGN_IN_FIELDS, SIGN_IN_MAX_USES,
    SIGN_IN_TTL_SECONDS,
};
use crate::deploy::{host_capability, DeployError, Runner};
use crate::targets::ComputeTarget;

/// One `constraints.credential_prefill[]` entry, in the shape the trajectory
/// destructures: a target, a field class, and a capability REFERENCE. No
/// secret is here, and none can be: the worker redeems the reference against
/// its own broker and zeroes the plaintext when the fill returns.
///
/// NO `authorization_id`. Weles derives what it will accept from the page
/// itself — `wsFillCredential` builds `{ purpose: 'weles.browser.fill',
/// resource: "origin:<origin>/<field class>" }` and nothing more — and
/// `assertCapability` compares the reference's `authorization_id` against
/// that expectation's, which is `undefined`. A reference carrying one is
/// therefore refused with `capability operation mismatch` before any
/// redemption. The Apple sign-in binds its pair to a guard id because its own
/// expectation is built with that id; copying the detail into this contract is
/// what made run 49cfed33 fail on the fill with zero agent steps.
pub fn prefill_entry(
    target: &str,
    field_class: &str,
    capability_id: &str,
    resource: &str,
) -> Value {
    json!({
        "target": target,
        "field_class": field_class,
        "capability": {
            "capability_id": capability_id,
            "purpose": FILL_PURPOSE,
            "resource": resource,
            "target": CAPABILITY_TARGET,
        },
    })
}

/// Issue the pair ON the redeeming host and return the prefill entries that
/// carry them.
///
/// Issued only after the action has been shown to be one the host accepts: a
/// capability is single-use and expires, so minting for a job that was about
/// to be refused would spend it on nothing. Issued on the TARGET because
/// redemption is a socket on the target: see
/// [`crate::deploy::host_capability`] for why the local precedent could not
/// work across hosts.
pub async fn issue_sign_in_prefill(
    target: &ComputeTarget,
    origin: &str,
    item: &str,
    scopes_file: &str,
    runner: &Runner,
) -> Result<SignInPrefill, DeployError> {
    let broker = host_capability::resolve(target, &weles_api_broker_files(), runner).await?;
    let routed = confirm_routed_item(target, &broker, origin, item, runner).await?;
    let scopes = host_scopes(target, scopes_file, runner).await?;
    let mut unconfirmed = Vec::new();
    let mut deferred = Vec::new();
    let mut agents = Vec::with_capacity(SIGN_IN_FIELDS.len());
    let mut entries = Vec::with_capacity(SIGN_IN_FIELDS.len());
    for ((fill_target, field_class), routed) in SIGN_IN_FIELDS.iter().zip(&routed) {
        let resource = fill_resource(origin, field_class);
        // The agent is the name this host's vault registers the worker's
        // workload key under for THIS coordinate. Read, never assumed: two
        // runs were denied for names that sounded right and were registered
        // nowhere.
        let agent = scope_consumer(&scopes, &routed.item, &routed.field).ok_or_else(|| {
            DeployError(format!(
                "{}: {scopes_file} registers no identity for {}/{}, so a capability for \
                 {resource} could only be issued to a name its vault does not know and its \
                 broker would deny",
                target.name, routed.item, routed.field
            ))
        })?;
        let capability_id = host_capability::issue(
            target,
            &broker,
            &host_capability::Issuance {
                agent,
                purpose: FILL_PURPOSE,
                resource: &resource,
                capability_target: CAPABILITY_TARGET,
                ttl_seconds: SIGN_IN_TTL_SECONDS,
                max_uses: SIGN_IN_MAX_USES,
                // Unbound on purpose: this consumer's expectation carries no
                // authorization id, so binding one guarantees a mismatch.
                authorization_id: None,
            },
            runner,
        )
        .await?;
        if !routed.readable {
            unconfirmed.push(format!("{}/{}", routed.item, routed.field));
        }
        agents.push(agent.to_string());
        let reference = prefill_entry(fill_target, field_class, &capability_id, &resource);
        // Only the identifier step is on the page the run opens. Every SSO this
        // action drives - Google, Apple, Microsoft - asks for the identifier
        // first and the secret on a page that does not exist yet, and a runtime
        // that fills every entry at load spends the secret's one-shot
        // capability on a field that cannot be there. Charless-mac-mini did
        // exactly that twice: both capabilities `spent` within two seconds of
        // the first page load, and the agent that reached the real password
        // field was denied for a capability nobody had used.
        //
        // So the identifier is prefilled and the rest are handed over unspent,
        // for the agent to redeem on the page that has the field. A runtime
        // that defers absent fields itself reaches the same place.
        if entries.is_empty() {
            entries.push(reference);
        } else {
            deferred.push(reference);
        }
    }
    Ok(SignInPrefill {
        entries,
        deferred,
        agents,
        unconfirmed,
    })
}

/// The references, and what the target could not confirm about them.
pub struct SignInPrefill {
    /// Filled by the runtime as soon as the page loads.
    pub entries: Vec<Value>,
    /// Handed to the agent unspent, for the step whose field appears later.
    pub deferred: Vec<Value>,
    /// The registered identity each capability was issued to, in the same
    /// order. Reported because it is the fact that decides whether redemption
    /// can verify at all.
    pub agents: Vec<String>,
    /// `item/field` coordinates the target's own listing could not open. Said
    /// out loud rather than treated as a refusal: the broker reads the item at
    /// redemption, and a channel session without gpg cannot answer for it.
    pub unconfirmed: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deploy::weles_browser_task::action::DEFAULT_ACTION;
    use crate::deploy::weles_browser_task::sign_in::exact_origin;
    use crate::deploy::weles_browser_task::task::BrowserTask;

    /// The exact JSON a prefill run submits. Every field here is one the
    /// trajectory destructures or the worker validates:
    /// `generic/browser_task.mjs` reads target/field_class/capability, and
    /// `wsFillCredential` requires purpose `weles.browser.fill` with resource
    /// `origin:<page origin>/<field class>` and target `weles`.
    #[test]
    fn a_prefill_run_puts_capability_references_on_the_wire_and_no_secret() {
        let origin = exact_origin("https://accounts.google.com").unwrap();
        let prefill: Vec<Value> = SIGN_IN_FIELDS
            .iter()
            .enumerate()
            .map(|(index, (target, field_class))| {
                prefill_entry(
                    target,
                    field_class,
                    &format!("{:064x}", index + 1),
                    &fill_resource(&origin, field_class),
                )
            })
            .collect();
        let task = BrowserTask {
            action: DEFAULT_ACTION,
            url: "https://accounts.google.com/",
            objective: "sign in and report the account",
            session_label: "oko-calendar",
            allow_login: true,
            headless: false,
            credential_prefill: prefill,
            login_item: None,
            account_id: None,
            fresh_profile: false,
        };
        let params = task.params();
        let entries = params["constraints"]["credential_prefill"]
            .as_array()
            .expect("prefill entries travel inside constraints");
        assert_eq!(entries.len(), 2, "{params}");

        assert_eq!(entries[0]["target"], json!("email"));
        assert_eq!(entries[0]["field_class"], json!("email"));
        assert_eq!(
            entries[0]["capability"]["resource"],
            json!("origin:https://accounts.google.com/email")
        );
        assert_eq!(entries[1]["target"], json!("password"));
        assert_eq!(entries[1]["field_class"], json!("password"));
        assert_eq!(
            entries[1]["capability"]["resource"],
            json!("origin:https://accounts.google.com/password")
        );
        for entry in entries {
            assert_eq!(entry["capability"]["purpose"], json!("weles.browser.fill"));
            assert_eq!(entry["capability"]["target"], json!("weles"));
            // NO authorization id. `wsFillCredential` builds its expectation as
            // `{ purpose, resource }`, and `assertCapability` compares the
            // reference's authorization_id against that expectation's
            // `undefined`: a bound reference is refused with `capability
            // operation mismatch` before anything is redeemed. Run 49cfed33
            // failed exactly there, on the fill, with zero agent steps.
            assert!(
                entry["capability"].get("authorization_id").is_none(),
                "{entry}"
            );
            // A reference and nothing else: four fields, none that could hold
            // a secret.
            let capability = entry["capability"].as_object().unwrap();
            assert_eq!(capability.len(), 4, "{entry}");
            for forbidden in ["value", "secret", "password", "email", "username"] {
                assert!(capability.get(forbidden).is_none(), "{entry}");
            }
        }
        // The sign-in does not disturb the rest of the schema.
        assert_eq!(params["constraints"]["no_login"], json!(false));
        assert_eq!(params["url"], json!("https://accounts.google.com/"));
    }
}
