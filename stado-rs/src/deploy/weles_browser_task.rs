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

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

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

/// The immutable action catalog shipped by the active Weles release.
pub const DEFAULT_ALLOWLIST_FILE: &str =
    "$HOME/weles/scripts/worker/deploy/weles-action-allowlist.txt";

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
        .map(super::service_env_file::effective_text)
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

/// The acquisition scopes catalog as the host registered it.
///
/// `stado host sync-acquisition-scopes` delivers the checked-in catalog to
/// `$HOME/.stado/files/` and runs `skarbiec token-register-acquisitions` from
/// THAT copy, so this staged file — not the release tree's — is the document
/// the vault's workload registrations were minted from.
pub const REGISTERED_SCOPES_FILE: &str = "$HOME/.stado/files/skarbiec-acquisition-scopes.conf";

/// One catalog row: a name a workload public key is registered under, and the
/// vault coordinate that grant covers.
#[derive(Debug, PartialEq, Eq)]
pub struct AcquisitionScope {
    pub consumer: String,
    pub item: String,
    pub field: String,
}

/// Every registered name in one catalog, in the order the file lists them.
///
/// `consumer|item|field` per line, `#` comments and blank lines ignored — the
/// shape `read_acquisition_catalog` parses on the host. A row this cannot read
/// is skipped rather than guessed at: the vault, not this parse, decides what
/// is registered, and a wrong guess here would name an identity that is not.
pub fn parse_scopes(body: &str) -> Vec<AcquisitionScope> {
    let mut scopes = Vec::new();
    for line in body.lines() {
        let row = line.trim();
        if row.is_empty() || row.starts_with('#') {
            continue;
        }
        let mut columns = row.split('|');
        let (Some(consumer), Some(item), Some(field), None) = (
            columns.next(),
            columns.next(),
            columns.next(),
            columns.next(),
        ) else {
            continue;
        };
        if consumer.is_empty() || item.is_empty() || field.is_empty() {
            continue;
        }
        scopes.push(AcquisitionScope {
            consumer: consumer.to_string(),
            item: item.to_string(),
            field: field.to_string(),
        });
    }
    scopes
}

/// The name a capability for one vault coordinate must be issued to.
///
/// Skarbiec authorises a redemption by the live vault token registering that
/// workload's Ed25519 key, and it looks that token up by the capability's
/// agent — by name, with no capability check of its own. On this fleet the
/// worker holds one key whose public half is registered under the catalog's
/// per-field consumer names, so the registered name for the coordinate being
/// filled is the only agent whose signature can verify.
///
/// Naming anything else is denied however correct the purpose, resource and
/// route are: run 18e7cc47 was refused for `weles-worker`, a constant copied
/// from the Apple sign-in, and run 47d89182 for `weles-credential-worker-local`,
/// the worker's own `SKARBIEC_WORKLOAD_ID` — that string labels the workload,
/// it is not a registration.
pub fn scope_consumer<'a>(
    scopes: &'a [AcquisitionScope],
    item: &str,
    field: &str,
) -> Option<&'a str> {
    scopes
        .iter()
        .find(|scope| scope.item == item && scope.field == field)
        .map(|scope| scope.consumer.as_str())
}

/// Read the catalog the host's workload registrations were minted from.
pub async fn host_scopes(
    target: &ComputeTarget,
    scopes_file: &str,
    runner: &Runner,
) -> Result<Vec<AcquisitionScope>, DeployError> {
    let fetched = service_file_fetch::fetch_file(target, scopes_file, runner).await?;
    if !fetched.ok() {
        return Err(DeployError(format!(
            "{}: could not read {scopes_file} to learn which identities its vault registers: {}",
            target.name, fetched.report.file_state
        )));
    }
    Ok(parse_scopes(&String::from_utf8_lossy(&fetched.content)))
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

/// The capability purpose a browser field fill redeems under.
///
/// Weles derives the expectation itself as
/// `{ purpose: 'weles.browser.fill', resource: "origin:<page origin>/<field
/// class>" }` and refuses anything else before it redeems, so these two
/// constants are not a convention this module chose — they are the worker's.
pub const FILL_PURPOSE: &str = "weles.browser.fill";

/// The capability target Weles requires on a reference it will redeem.
pub const CAPABILITY_TARGET: &str = "weles";

/// Skarbiec's own maximum, and what the Apple sign-in asks for. A browser run
/// is held open for its whole duration, so a shorter window would expire
/// mid-flow; single use is what makes the exposure one fill.
pub const SIGN_IN_TTL_SECONDS: &str = "3600";
pub const SIGN_IN_MAX_USES: &str = "1";

/// The pair a form sign-in needs: the fill target handed to Weles, and the
/// field class that target must agree with.
///
/// The targets are not decoration. Weles refuses a fill whose target does not
/// match the field class's own hint — `/email|e-mail/` and
/// `/password|passcode|secret/` — before redeeming, so a pair that disagreed
/// would burn a one-shot capability on `credential field class mismatch`.
pub const SIGN_IN_FIELDS: [(&str, &str); 2] = [("email", "email"), ("password", "password")];

/// The capability state the Weles API's own broker serves.
///
/// From that product's launcher, `launch-weles-api-mac.sh:104`: the broker on
/// `$HOME/.stado/run/weles-api-capability.sock` — the socket the worker's
/// `SKARBIEC_CAP_SOCKET` names — is started with these files, not with
/// Skarbiec's vault-adjacent defaults. A capability issued into the default
/// state is invisible to it.
pub const WELES_API_CAPABILITY_FILE: &str = "$HOME/.stado/weles-api-capabilities.json";

/// The route table that same broker resolves against
/// (`launch-weles-api-mac.sh:105`).
///
/// Note for anyone declaring a route here: that launcher REINSTALLS this file
/// from `weles/scripts/worker/deploy/weles-capability-routes.json` on every
/// start, so a route declared on the host lasts until the unit next launches.
/// The durable place for a new one is that checked-in file.
pub const WELES_API_ROUTES_FILE: &str = "$HOME/.stado/weles-api-capability-routes.json";

/// The broker instance a Weles browser fill is issued into.
pub fn weles_api_broker_files() -> super::host_capability::BrokerFiles<'static> {
    super::host_capability::BrokerFiles {
        capability_file: Some(WELES_API_CAPABILITY_FILE),
        routes_file: Some(WELES_API_ROUTES_FILE),
    }
}

/// One page origin, in the exact form Weles compares against.
///
/// Weles builds its expectation from `new URL(page.url()).origin`, so anything
/// carrying a path, a query, a fragment or userinfo could never match and would
/// be spent finding that out. The HTTP(S) sentence is the worker's own.
pub fn exact_origin(raw: &str) -> Result<String, DeployError> {
    let parsed = url::Url::parse(raw)
        .map_err(|error| DeployError(format!("--sign-in-origin is not a URL: {error}")))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(DeployError(
            "credential fill requires an HTTP(S) origin".to_string(),
        ));
    }
    if parsed.username() != "" || parsed.password().is_some() {
        return Err(DeployError(
            "--sign-in-origin must not carry embedded credentials".to_string(),
        ));
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(DeployError(
            "credential fill requires an HTTP(S) origin".to_string(),
        ));
    }
    if !matches!(parsed.path(), "" | "/") || parsed.query().is_some() || parsed.fragment().is_some()
    {
        return Err(DeployError(format!(
            "--sign-in-origin must be a bare origin such as https://accounts.google.com, \
             with no path, query or fragment: {raw}"
        )));
    }
    Ok(parsed.origin().ascii_serialization())
}

/// The resource string for one field class on one origin.
pub fn fill_resource(origin: &str, field_class: &str) -> String {
    format!("origin:{origin}/{field_class}")
}

/// Which vault item the broker would actually hand this resource to.
///
/// The caller names the item it believes holds the account; the route table
/// decides which item is really read. Those two disagreeing is the failure
/// Skarbiec's own route table was built for — a route pointing somewhere the
/// operator did not mean is indistinguishable from a working one until a login
/// needs it. So the claim is checked before a capability exists.
pub fn routed_item(routes: &Value, resource: &str) -> Result<RoutedField, DeployError> {
    let rows = routes
        .get("routes")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("skarbiec routes list returned no routes".to_string()))?;
    let row = rows
        .iter()
        .find(|row| row.get("resource").and_then(Value::as_str) == Some(resource))
        .ok_or_else(|| {
            DeployError(format!(
                "no capability route maps {resource} to a vault field"
            ))
        })?;
    let item = row
        .get("item")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let field = row
        .get("field")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if item.is_empty() || field.is_empty() {
        return Err(DeployError(format!(
            "capability route for {resource} must name an item and a field"
        )));
    }
    Ok(RoutedField {
        item,
        field,
        // Advisory, NOT a gate. `routes list` answers these as the process
        // that asked, and over a host channel that process has no gpg: every
        // route on charless-mac-mini reports `does not open: spawn gpg` while
        // the broker service on that same host reads those items fine.
        // Refusing on them would refuse every real sign-in for the wrong
        // reason, and redemption is where the item is actually read.
        readable: row
            .get("item_present")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && row
                .get("field_present")
                .and_then(Value::as_bool)
                .unwrap_or(false),
    })
}

/// One route as the target answered it.
#[derive(Debug)]
pub struct RoutedField {
    pub item: String,
    pub field: String,
    /// Whether the ASKING process could open the item and find the field.
    /// Advisory only — see [`routed_item`].
    pub readable: bool,
}

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

/// The sentence a caller reads when the redeeming host has no route for the
/// origin they asked to sign in on.
///
/// It names both exact resources, the item they must map to, and the command
/// that declares them. Declaring which credential a login form receives is a
/// decision that belongs in a command someone ran on purpose, so this refuses
/// rather than creating them.
pub fn missing_route_sentence(host: &str, origin: &str, item: &str, detail: &str) -> String {
    let declarations: Vec<String> = SIGN_IN_FIELDS
        .iter()
        .map(|(_, field_class)| {
            format!(
                "stado host capability-route {host} --resource {} --item {item} --field {} \
                 --reason <why>",
                fill_resource(origin, field_class),
                vault_field_for(field_class),
            )
        })
        .collect();
    format!(
        "{host} cannot fill a sign-in on {origin}: {detail}. It needs both of \
         {} and {}, mapped to vault item {item} fields {} and {}. Declare them there, on \
         purpose, then run this again:\n  {}",
        fill_resource(origin, SIGN_IN_FIELDS[0].1),
        fill_resource(origin, SIGN_IN_FIELDS[1].1),
        vault_field_for(SIGN_IN_FIELDS[0].1),
        vault_field_for(SIGN_IN_FIELDS[1].1),
        declarations.join("\n  "),
    )
}

/// The vault field a Weles login contract keeps each class in.
///
/// Skarbiec's own login shape, and the one every `origin:` route in the fleet
/// already uses: `platform-admin-cloudflare/username` and `/password`,
/// `platform-admin-appstore/username` and `/password`. An email field class
/// reads the item's `username`.
pub fn vault_field_for(field_class: &str) -> &'static str {
    match field_class {
        "email" => "username",
        _ => "password",
    }
}

/// Confirm the redeeming host routes both resources to the item the caller
/// named.
///
/// Returns what each resource routes to, in `SIGN_IN_FIELDS` order: the vault
/// field decides which registered identity the capability must be issued to,
/// and `readable` is the part the caller says out loud. Whether the broker can
/// open the item is the broker's business at
/// redemption; whether the route points at the item the operator named is this
/// command's business, and that is what is enforced here.
pub async fn confirm_routed_item(
    target: &ComputeTarget,
    broker: &super::host_capability::RemoteBroker,
    origin: &str,
    item: &str,
    runner: &Runner,
) -> Result<Vec<RoutedField>, DeployError> {
    let routes = super::host_capability::routes(target, broker, runner).await?;
    let mut confirmed = Vec::with_capacity(SIGN_IN_FIELDS.len());
    for (_, field_class) in SIGN_IN_FIELDS {
        let resource = fill_resource(origin, field_class);
        let routed = routed_item(&routes, &resource).map_err(|error| {
            DeployError(missing_route_sentence(
                &target.name,
                origin,
                item,
                &error.to_string(),
            ))
        })?;
        if routed.item != item {
            return Err(DeployError(format!(
                "{}: {resource} routes to vault item {} field {}, not to {item}; \
                 the item that would be read is the one the route names",
                target.name, routed.item, routed.field
            )));
        }
        confirmed.push(routed);
    }
    Ok(confirmed)
}

/// Issue the pair ON the redeeming host and return the prefill entries that
/// carry them.
///
/// Issued only after the action has been shown to be one the host accepts: a
/// capability is single-use and expires, so minting for a job that was about
/// to be refused would spend it on nothing. Issued on the TARGET because
/// redemption is a socket on the target: see
/// [`super::host_capability`] for why the local precedent could not work
/// across hosts.
pub async fn issue_sign_in_prefill(
    target: &ComputeTarget,
    origin: &str,
    item: &str,
    scopes_file: &str,
    runner: &Runner,
) -> Result<SignInPrefill, DeployError> {
    let broker = super::host_capability::resolve(target, &weles_api_broker_files(), runner).await?;
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
        let capability_id = super::host_capability::issue(
            target,
            &broker,
            &super::host_capability::Issuance {
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
    /// The parameter object, in the exact shape
    /// `host weles-image-inspect` already sends for this action — so the two
    /// callers of `generic_browser_task` cannot disagree about its schema.
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

/// Everything one completed task reports back.
pub struct TaskOutcome {
    pub run_id: String,
    pub ok: bool,
    pub exit_code: Option<i64>,
    pub result: Value,
    pub profile: Option<Value>,
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
        if let Some(profile) = &self.profile {
            object.insert("profile".to_string(), profile.clone());
        }
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
pub async fn submit(
    target: &str,
    task: &BrowserTask<'_>,
    flow_name: Option<&str>,
    credential_deferred: &[Value],
) -> Result<TaskOutcome, DeployError> {
    let admission = weles_capture::resolve_admission(target).await?;
    let channel = weles_capture::open_channel(&admission).await?;
    let payload = weles_capture::observe_action_payload(
        &channel,
        task.action,
        task.params_with(flow_name, credential_deferred),
        task.account_id,
        task.fresh_profile,
    )
    .await?;
    let run_id = payload
        .get("run_id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let profile = if task.fresh_profile {
        task.account_id.map(|account_id| {
            let directory_key = hex::encode(Sha256::digest(account_id.as_bytes()));
            let platform = task.action.split('_').next().unwrap_or("unknown");
            json!({
                "mode": "fresh",
                "account_id": account_id,
                "directory_key": directory_key,
                "directory": format!(
                    "$HOME/.local/state/weles/browser-profiles/{platform}/chromium/{directory_key}"
                ),
            })
        })
    } else {
        None
    };
    Ok(TaskOutcome {
        ok: payload.get("ok").and_then(Value::as_bool).unwrap_or(false),
        exit_code: payload.get("exitCode").and_then(Value::as_i64),
        result: payload.get("result").cloned().unwrap_or(Value::Null),
        run_id,
        profile,
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
        // The schema stays the one `weles-image-inspect` already sends.
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

    /// The fill targets must satisfy Weles's own field-class hints, or the
    /// worker throws `credential field class mismatch` and the one-shot
    /// capability is already spent.
    #[test]
    fn the_fill_targets_match_the_hints_weles_checks_before_redeeming() {
        let hints = [("email", "email"), ("password", "password")];
        for ((target, field_class), (expect_target, expect_class)) in
            SIGN_IN_FIELDS.iter().zip(hints)
        {
            assert_eq!(*target, expect_target);
            assert_eq!(*field_class, expect_class);
            assert!(target.to_lowercase().contains(field_class));
        }
    }

    #[test]
    fn an_origin_that_weles_could_never_match_is_refused_before_anything_is_minted() {
        // The worker's own sentence for a non-HTTP(S) page.
        let said = exact_origin("ftp://accounts.google.com")
            .unwrap_err()
            .to_string();
        assert_eq!(said, "credential fill requires an HTTP(S) origin");

        // Weles compares `new URL(page.url()).origin`, which carries no path.
        let said = exact_origin("https://accounts.google.com/signin")
            .unwrap_err()
            .to_string();
        assert!(said.contains("bare origin"), "{said}");
        assert!(said.contains("no path, query or fragment"), "{said}");

        let said = exact_origin("https://user:pw@accounts.google.com")
            .unwrap_err()
            .to_string();
        assert!(said.contains("embedded credentials"), "{said}");

        // A trailing slash is the origin itself and is accepted.
        assert_eq!(
            exact_origin("https://accounts.google.com/").unwrap(),
            "https://accounts.google.com"
        );
        // A non-default port belongs to the origin Weles would compute.
        assert_eq!(
            exact_origin("http://localhost:8080").unwrap(),
            "http://localhost:8080"
        );
    }

    /// A route that does not exist is refused; a route whose readability the
    /// ASKING process could not confirm is reported, not refused. Over a host
    /// channel every route on charless-mac-mini answers `does not open: spawn
    /// gpg` because that session has no gpg, while the broker service on the
    /// same host reads those items fine — so refusing on that boolean would
    /// refuse every real sign-in for a reason that is about the wrong process.
    #[test]
    fn a_missing_route_is_refused_and_an_unconfirmable_one_is_only_reported() {
        let routes = json!({
            "consumer": null,
            "routes": [
                {
                    "resource": "origin:https://accounts.google.com/email",
                    "item": "weles-google-sso-login",
                    "field": "username",
                    "item_present": true,
                    "field_present": true,
                },
                {
                    "resource": "origin:https://accounts.google.com/password",
                    "item": "weles-google-sso-login",
                    "field": "password",
                    "item_present": false,
                    "field_present": false,
                },
                {
                    "resource": "origin:https://dash.cloudflare.com/email",
                    "item": "",
                    "field": "",
                    "item_present": false,
                    "field_present": false,
                },
            ],
        });

        let routed = routed_item(&routes, "origin:https://accounts.google.com/email").unwrap();
        assert_eq!(routed.item, "weles-google-sso-login");
        assert_eq!(routed.field, "username");
        assert!(routed.readable);

        // The `spawn gpg` shape: mapped, but this process could not open it.
        // Still Ok, with readable false for the caller to say out loud.
        let routed = routed_item(&routes, "origin:https://accounts.google.com/password").unwrap();
        assert_eq!(routed.item, "weles-google-sso-login");
        assert_eq!(routed.field, "password");
        assert!(!routed.readable);

        // A table entry that names no coordinates is broken, not advisory.
        let said = routed_item(&routes, "origin:https://dash.cloudflare.com/email")
            .unwrap_err()
            .to_string();
        assert!(said.contains("must name an item and a field"), "{said}");

        // Skarbiec's own sentence for a resource with no route at all.
        let said = routed_item(&routes, "origin:https://example.com/email")
            .unwrap_err()
            .to_string();
        assert_eq!(
            said,
            "no capability route maps origin:https://example.com/email to a vault field"
        );
    }

    /// A host with no route for the origin must be told exactly what to
    /// declare, on which host, against which item — and must NOT have it
    /// declared for it. This sentence is the whole interface between "the run
    /// cannot work" and "an operator decided which credential this form gets".
    #[test]
    fn a_host_without_the_routes_is_told_exactly_what_to_declare() {
        let said = missing_route_sentence(
            "charless-mac-mini",
            "https://accounts.google.com",
            "weles-google-sso-login",
            "no capability route maps origin:https://accounts.google.com/email to a vault field",
        );
        assert!(said.contains("charless-mac-mini"), "{said}");
        assert!(
            said.contains("origin:https://accounts.google.com/email"),
            "{said}"
        );
        assert!(
            said.contains("origin:https://accounts.google.com/password"),
            "{said}"
        );
        assert!(said.contains("weles-google-sso-login"), "{said}");
        // The vault field names a Weles login contract actually uses, the same
        // ones every existing `origin:` route in the fleet maps to.
        assert!(said.contains("username"), "{said}");
        assert!(said.contains("password"), "{said}");
        // And the command that declares them, on that host, with a reason.
        assert!(
            said.contains("stado host capability-route charless-mac-mini --resource"),
            "{said}"
        );
        assert!(said.contains("--reason"), "{said}");
    }

    #[test]
    fn an_email_field_class_reads_the_items_username() {
        assert_eq!(vault_field_for("email"), "username");
        assert_eq!(vault_field_for("password"), "password");
    }

    #[test]
    fn the_identity_for_a_coordinate_is_the_one_the_catalog_registers_for_it() {
        // charless-mac-mini's own catalog, four rows of it: one consumer per
        // (item, field), which is why the agent cannot be a single per-host
        // name.
        let body = "# consumer|item|field\n\
                    \n\
                    weles-gmail-client-username|weles-gmail-login|username\n\
                    weles-google-sso-client-username|weles-google-sso-login|username\n\
                    weles-google-sso-client-password|weles-google-sso-login|password\n";
        let scopes = parse_scopes(body);
        assert_eq!(scopes.len(), 3);
        assert_eq!(
            scope_consumer(&scopes, "weles-google-sso-login", "username"),
            Some("weles-google-sso-client-username")
        );
        assert_eq!(
            scope_consumer(&scopes, "weles-google-sso-login", "password"),
            Some("weles-google-sso-client-password")
        );
    }

    #[test]
    fn a_coordinate_the_catalog_never_registers_has_no_identity_to_issue_to() {
        // The two denials this replaced: names that sound like the worker but
        // register nothing. An unregistered coordinate must answer None so the
        // caller refuses before spending a capability the broker would deny.
        let scopes =
            parse_scopes("weles-google-sso-client-username|weles-google-sso-login|username\n");
        assert_eq!(
            scope_consumer(&scopes, "weles-google-sso-login", "totp_secret"),
            None
        );
        assert_eq!(scope_consumer(&scopes, "weles-worker", "username"), None);
    }

    #[test]
    fn a_row_this_cannot_read_is_skipped_rather_than_guessed_at() {
        let scopes = parse_scopes(
            "# comment\n\
             \n\
             two|columns\n\
             four|too|many|columns\n\
             |weles-google-sso-login|username\n\
             weles-google-sso-client-username|weles-google-sso-login|username\n",
        );
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].consumer, "weles-google-sso-client-username");
    }
}
