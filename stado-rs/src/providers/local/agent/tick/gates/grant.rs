//! The agent's own Skarbiec grant, settled by the agent until it lives until
//! revoked.
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

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::cli::secrets::{launcher_json, skarbiec_launcher};
use crate::credential_store::grant::GrantLifetime;

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

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// The exact renewal invocation, as one sentence, so the log and a refusal
/// name what was run.
fn renewal_command(consumer: &str, capabilities: &str, token_file: &str) -> String {
    format!(
        "skarbiec grant issue {consumer} --capabilities {capabilities} --replace-capabilities \
         --token-file {token_file} --until-revoked"
    )
}

/// The consumer's grant in the owner vault: when it ends, and the
/// `action:item#field` capabilities it carries. `None` when the vault holds
/// no grant for the consumer.
fn grant_record(listing: &Value, consumer: &str) -> Option<(u64, Vec<String>)> {
    let grants = listing
        .get("grants")
        .and_then(Value::as_array)
        .or_else(|| listing.as_array())?;
    grants
        .iter()
        .filter(|grant| grant.get("consumer").and_then(Value::as_str) == Some(consumer))
        .filter_map(|grant| {
            let expires_at = grant.get("expires_at").and_then(Value::as_u64)?;
            let capabilities = grant
                .get("capabilities")
                .and_then(Value::as_array)
                .map(|entries| {
                    entries
                        .iter()
                        .filter_map(|entry| {
                            let action = entry.get("action").and_then(Value::as_str)?;
                            let item = entry.get("item").and_then(Value::as_str)?;
                            Some(match entry.get("field").and_then(Value::as_str) {
                                Some(field) if !field.is_empty() => {
                                    format!("{action}:{item}#{field}")
                                }
                                _ => format!("{action}:{item}"),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some((expires_at, capabilities))
        })
        .max_by_key(|(expires_at, _)| *expires_at)
}

/// The bond this vault replicates, when `skarbiec bond status` says this
/// vault is a replica: the authority lives on that host, and a grant written
/// here is overwritten by the next pull.
fn replica_of(launcher: &Path, vault: &Path) -> Option<String> {
    let status = launcher_json(launcher, vault, &["bond", "status"]).ok()?;
    status
        .as_array()?
        .iter()
        .find(|bond| bond.get("role").and_then(Value::as_str) == Some("replica"))
        .and_then(|bond| bond.get("bond").and_then(Value::as_str))
        .map(str::to_owned)
}

/// Look at the agent's grant on every tick until it lives until revoked, and
/// re-issue it that way when it does not. A look's sentences are logged when
/// they differ from the previous look's; a host without the owner vault logs
/// nothing, because there is nothing it could do.
pub(crate) async fn renew_if_due(log_fn: &mut dyn FnMut(&str)) {
    if SETTLED.load(Ordering::Relaxed) {
        return;
    }
    let mut sentences = Vec::new();
    let outcome = renew(false, &mut |line| sentences.push(line.to_string())).await;
    if outcome != RenewOutcome::Failed {
        SETTLED.store(true, Ordering::Relaxed);
    }
    let mut last = LAST_SENTENCES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if *last != sentences {
        for sentence in &sentences {
            log_fn(sentence);
        }
        *last = sentences;
    }
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

/// The renewal itself. `force` renews a grant that is not yet due, which is
/// what `stado credentials grant renew --force` asks for; the tick never does.
/// Every sentence goes to `log_fn`, including why nothing was done.
pub(crate) async fn renew(force: bool, log_fn: &mut dyn FnMut(&str)) -> RenewOutcome {
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
    let at = now();
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
    let record = grant_record(&listing, consumer);
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
    // has never held starts from the declaration, whose `role#field` entries
    // are translated to the items playing those roles right now.
    let issued = record
        .as_ref()
        .map(|(_, capabilities)| capabilities.clone())
        .unwrap_or_default();
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
    if let Some(bond) = replica_of(&launcher, &vault) {
        // This vault follows another host's, and that host holds the agent's
        // bearer at the path bootstrap provisioned it to; the grant is
        // reissued there, on the host, and comes back with the next pull.
        let target = match crate::deploy::host_channel::canonical_target(&bond).await {
            Ok(target) => target,
            Err(error) => {
                log_fn(&format!(
                    "agent grant: {consumer} {state} and the authoritative vault's host {bond} cannot be resolved: {error}"
                ));
                return RenewOutcome::Failed;
            }
        };
        let runner = crate::deploy::production_runner();
        return match crate::deploy::service::remint_consumer_grant_on_host(
            &target,
            consumer,
            &capabilities,
            REMOTE_AGENT_TOKEN_FILE,
            REMOTE_VAULT_FILE,
            GrantLifetime::UntilRevoked,
            consumer,
            &runner,
        )
        .await
        {
            Ok(report) if report.succeeded("grant_synced") => {
                log_fn(&format!(
                    "agent grant: {consumer} {state}; re-issued on {bond} until revoked, this replica pulls it within its sync interval"
                ));
                RenewOutcome::Renewed
            }
            Ok(report) => {
                log_fn(&format!(
                    "agent grant: {consumer} {state} and renewal on {bond} failed: {}",
                    report.failure()
                ));
                RenewOutcome::Failed
            }
            Err(error) => {
                log_fn(&format!(
                    "agent grant: {consumer} {state} and renewal on {bond} could not run: {error}"
                ));
                RenewOutcome::Failed
            }
        };
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let output = crate::wait::output(
        &mut Command::new(&launcher)
            .args(["grant", "issue", consumer, "--capabilities", &capabilities])
            .args(["--replace-capabilities", "--token-file", token_file])
            .arg("--until-revoked")
            .env("SKARBIEC_VAULT_FILE", &vault)
            .env("GNUPGHOME", format!("{home}/.gnupg"))
            .env(
                "PATH",
                "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            )
            .env_remove("SKARBIEC_UNLOCK")
            .env_remove("SKARBIEC_UNLOCK_FILE"),
    );
    match output {
        Ok(output) if output.status.success() => {
            let reissued = launcher_json(&launcher, &vault, &["grant", "list"])
                .ok()
                .and_then(|listing| grant_record(&listing, consumer))
                .map(|(expires_at, _)| expires_at);
            log_fn(&format!(
                "agent grant: {consumer} {state}; re-issued until revoked, the vault now records \
                 it {}",
                match reissued {
                    Some(expires_at) if GrantLifetime::is_until_revoked(expires_at) =>
                        "until revoked".to_string(),
                    Some(expires_at) => format!("ending at {expires_at}"),
                    None => "unread".to_string(),
                }
            ));
            RenewOutcome::Renewed
        }
        Ok(output) => {
            log_fn(&format!(
                "agent grant: {consumer} {state} and renewal failed: `{}` exited {}: {}",
                renewal_command(consumer, &capabilities, token_file),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
            RenewOutcome::Failed
        }
        Err(error) => {
            log_fn(&format!(
                "agent grant: {consumer} {state} and renewal could not start: `{}`: {error}",
                renewal_command(consumer, &capabilities, token_file)
            ));
            RenewOutcome::Failed
        }
    }
}
