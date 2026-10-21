//! The agent's own Skarbiec grant, settled by the agent until it lives until
//! revoked, and every consumer grant the service directory declares, kept
//! until revoked (`declared`).
//!
//! A workload that declares `secret_env` is claimed only by a host whose agent
//! consumer can list the items playing those roles, and the consumer can list
//! them only while its grant lives. The agent re-issues its grant with the
//! bearer it already holds, so a grant with an end would only ever be pushed
//! forward: the end protected nothing a revocation does not, and a renewal
//! that missed it was an outage. The grant is therefore issued
//! `--until-revoked`, and a grant the vault still records with an end — one
//! issued before this, for thirty days — is re-issued once that way.
//!
//! Only a host that holds the owner vault can issue a grant, so this runs on
//! the control plane's own agent and is a no-op elsewhere; a remote agent's
//! grant is provisioned by the control plane at bootstrap. A first grant reads
//! the roles `agent.skarbiec.secret_fields` declares — the same declaration
//! `fleet doctor` checks the live grant against. Once the grant is seen to
//! live until revoked the tick stops looking for the life of the process, so
//! an operator's `skarbiec grant revoke` stands until the agent starts again
//! or `stado credentials grant renew` is run.

mod bearer;
mod declared;
mod issue;
mod vault;

pub(crate) use declared::{keep_declared, sentences as declared_sentences};

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::cli::secrets::{launcher_json, skarbiec_launcher};
use crate::credential_store::grant::GrantLifetime;
use vault::{grant_record, now, replica_of};

/// Where bootstrap put the agent's bearer on a fleet host, and the vault
/// every consumer grant on that host is minted against; the same two paths
/// `service grant sync` and web deploy use.
const REMOTE_AGENT_TOKEN_FILE: &str = "$HOME/.stado/local-agent-skarbiec-token";
const REMOTE_VAULT_FILE: &str = "$HOME/.stado/skarbiec.vault.json";

/// Whether this process has seen its grant live until revoked, or seen that
/// this host has nothing to issue; the tick looks no further once it has.
static SETTLED: AtomicBool = AtomicBool::new(false);
/// The sentences the last unsettled look logged, so a look that fails the
/// same way on every tick is logged once, and again when its sentence changes.
static LAST_SENTENCES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The exact renewal invocation, as one sentence, so the log and a refusal
/// name what was run.
fn renewal_command(consumer: &str, capabilities: &str, token_file: &str) -> String {
    format!(
        "skarbiec grant issue {consumer} --capabilities {capabilities} --replace-capabilities \
         --token-file {token_file} --until-revoked"
    )
}

/// Look at the agent's grant on every tick until it lives until revoked, and
/// re-issue it that way when it does not; then keep every declared consumer
/// grant the owner vault records with an end. A look's sentences are logged
/// when they differ from the previous look's; a host without the owner vault
/// logs nothing, because there is nothing it could do.
pub(crate) async fn renew_if_due(log_fn: &mut dyn FnMut(&str)) {
    if !SETTLED.load(Ordering::Relaxed) {
        let mut sentences = Vec::new();
        let outcome = renew_own(false, &mut |line| sentences.push(line.to_string())).await;
        if outcome != RenewOutcome::Failed {
            SETTLED.store(true, Ordering::Relaxed);
        }
        let mut last = match LAST_SENTENCES.lock() {
            Ok(last) => last,
            // A look that panicked while holding the list left whole
            // sentences in it; they are still the last ones logged.
            Err(poisoned) => poisoned.into_inner(),
        };
        if *last != sentences {
            for sentence in &sentences {
                log_fn(sentence);
            }
            *last = sentences;
        }
    }
    declared::keep_if_due(log_fn).await;
}

/// What one renewal pass did, so a caller decides on the outcome itself and
/// never on the wording of the sentence it logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RenewOutcome {
    /// The grant was reissued.
    Renewed,
    /// Nothing was due, or this host cannot issue grants and has nothing to do.
    NothingToDo,
    /// A step failed; the logged sentence names it.
    Failed,
}

/// What `stado credentials grant renew` asks for: the agent's own grant
/// (`force` renews it when it is not yet due, which the tick never does), and
/// then every declared consumer grant: kept until revoked and bound to its
/// declared bearer.
/// Every sentence goes to `log_fn`, including why nothing was done; a failure
/// of either half is the outcome.
pub(crate) async fn renew(force: bool, log_fn: &mut dyn FnMut(&str)) -> RenewOutcome {
    let own = renew_own(force, log_fn).await;
    let report = keep_declared().await;
    for sentence in declared_sentences(&report) {
        log_fn(&sentence);
    }
    let declared_failed = match &report {
        Ok(report) => !report.refused.is_empty(),
        Err(declared::NotHere::NotTheOwner(_)) => false,
        Err(declared::NotHere::Unreadable(_)) => true,
    };
    match (own, &report) {
        (RenewOutcome::Failed, _) => RenewOutcome::Failed,
        _ if declared_failed => RenewOutcome::Failed,
        (RenewOutcome::Renewed, _) => RenewOutcome::Renewed,
        (_, Ok(report)) if !report.kept.is_empty() || !report.rebound.is_empty() => {
            RenewOutcome::Renewed
        }
        _ => RenewOutcome::NothingToDo,
    }
}

/// The agent's own grant: re-issued until revoked when the vault records an
/// end for it, or when `force` asks.
async fn renew_own(force: bool, log_fn: &mut dyn FnMut(&str)) -> RenewOutcome {
    let consumer = crate::config::agent_skarbiec_consumer();
    let token_file = crate::config::agent_skarbiec_token_file();
    if consumer.is_empty() || token_file.is_empty() {
        log_fn(
            "agent grant: agent.skarbiec.consumer or agent.skarbiec.token_file is not configured",
        );
        return RenewOutcome::Failed;
    }
    if !Path::new(token_file).is_file() {
        log_fn(&format!(
            "agent grant: the token file {token_file} is not a regular file on this host"
        ));
        return RenewOutcome::Failed;
    }
    let at = match now() {
        Ok(at) => at,
        Err(error) => {
            log_fn(&format!("agent grant: {error}"));
            return RenewOutcome::Failed;
        }
    };
    let vault = match crate::credential_store::owner::vault() {
        Ok(vault) => vault,
        Err(error) => {
            if force {
                log_fn(&format!(
                    "agent grant: this host holds no owner vault, so nothing can be issued here: {error}"
                ));
            }
            return RenewOutcome::NothingToDo;
        }
    };
    let launcher = match skarbiec_launcher() {
        Ok(launcher) => launcher,
        Err(error) => {
            if force {
                log_fn(&format!("agent grant: {error}"));
            }
            return RenewOutcome::Failed;
        }
    };
    let listing = match launcher_json(&launcher, &vault, &["grant", "list"]) {
        Ok(listing) => listing,
        Err(error) => {
            log_fn(&format!(
                "agent grant: cannot read the owner vault's grants for {consumer}: {error}"
            ));
            return RenewOutcome::Failed;
        }
    };
    let record = match grant_record(&listing, consumer) {
        Ok(record) => record,
        Err(error) => {
            log_fn(&format!("agent grant: {error}"));
            return RenewOutcome::Failed;
        }
    };
    let expiry = record.as_ref().map(|(expires_at, _)| *expires_at);
    let due = !expiry.is_some_and(GrantLifetime::is_until_revoked);
    if !due && !force {
        return RenewOutcome::NothingToDo;
    }
    if !due {
        log_fn(&format!(
            "agent grant: {consumer} already lives until revoked; re-issuing it because it was \
             asked for"
        ));
    }
    // The grant keeps the capabilities it was issued with: the issuer chose
    // them, and a renewal that narrowed them to the field-level declaration
    // would silently drop what a release recipe reads. Only a grant the vault
    // has never held, or one holding none, starts from the declaration, whose
    // `role#field` entries are translated to the items playing those roles
    // right now.
    let issued = match &record {
        Some((_, capabilities)) => capabilities.clone(),
        None => Vec::new(),
    };
    let capabilities = if issued.is_empty() {
        match launcher_json(&launcher, &vault, &["list"])
            .map_err(|error| error.to_string())
            .and_then(|listing| {
                serde_json::from_value::<Vec<crate::skarbiec::ItemInfo>>(listing)
                    .map_err(|error| error.to_string())
            })
            .and_then(|items| {
                crate::skarbiec::roles::read_capabilities(
                    &items,
                    crate::config::agent_skarbiec_secret_fields(),
                )
            }) {
            Ok(declared) => declared,
            Err(error) => {
                log_fn(&format!(
                    "agent grant: {consumer} is absent and its declared roles cannot be granted: {error}"
                ));
                return RenewOutcome::Failed;
            }
        }
    } else {
        issued
    }
    .join(",");
    if capabilities.is_empty() {
        log_fn(&format!(
            "agent grant: {consumer} has no issued capabilities and declares no agent.skarbiec.secret_fields, so there is nothing to renew"
        ));
        return RenewOutcome::NothingToDo;
    }
    let state = match expiry {
        None => "is absent from the owner vault".to_string(),
        Some(expires_at) if GrantLifetime::is_until_revoked(expires_at) => {
            "lives until revoked".to_string()
        }
        Some(expires_at) if expires_at <= at => format!("expired at {expires_at}"),
        Some(expires_at) => format!("ends at {expires_at}"),
    };
    match replica_of(&launcher, &vault) {
        Ok(Some(bond)) => {
            return issue::on_authority(&bond, consumer, &capabilities, &state, log_fn).await
        }
        Ok(None) => {}
        Err(error) => {
            log_fn(&format!("agent grant: {error}"));
            return RenewOutcome::Failed;
        }
    }
    issue::here(
        &launcher,
        &vault,
        consumer,
        token_file,
        &capabilities,
        &state,
        log_fn,
    )
}
