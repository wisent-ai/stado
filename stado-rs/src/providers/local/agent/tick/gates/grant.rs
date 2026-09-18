//! The agent's own Skarbiec grant, renewed by the agent before it runs out.
//!
//! A workload that declares `secret_env` is claimed only by a host whose agent
//! consumer can list those items, and the consumer can list them only while
//! its grant lives. The grant is issued with a lifetime and nothing renewed
//! it: on 2026-09-17 the laptop's `stado-local-agent` grant expired at
//! 13:11:57 and every darwin release build of Jeden sat in the queue for the
//! rest of the day while the agent declined it every twenty seconds. The
//! operator's rule is that no such renewal is a person's chore, so the agent
//! that needs the grant keeps it alive.
//!
//! Only a host that holds the owner vault can issue a grant, so this runs on
//! the control plane's own agent and is a no-op elsewhere; a remote agent's
//! grant is provisioned by the control plane at bootstrap. The capabilities
//! are the ones `agent.skarbiec.secret_fields` declares — the same
//! declaration `fleet doctor` checks the live grant against — and the
//! lifetime is the thirty days `service grant-sync` gives every other
//! consumer. Renewal happens while a third of that lifetime is still left,
//! so one missed tick never becomes an expired grant.

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::cli::secrets::{launcher_json, skarbiec_launcher};

/// Thirty days, `service grant-sync`'s default for every consumer grant.
const GRANT_TTL_SECONDS: u64 = 2_592_000;
/// Renew once less than this much of the lifetime remains: ten days.
const RENEWAL_WINDOW_SECONDS: u64 = GRANT_TTL_SECONDS / 3;
/// How often the grant is looked at; a look is one `skarbiec grant list`.
const CHECK_INTERVAL_SECONDS: u64 = 600;

static LAST_CHECK: AtomicU64 = AtomicU64::new(0);

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
         --token-file {token_file} --ttl-seconds {GRANT_TTL_SECONDS}"
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

/// Look at the agent's grant and renew it when it is about to end or has
/// ended. Every outcome is one log line; a host without the owner vault
/// logs nothing, because there is nothing it could do.
pub(crate) fn renew_if_due(log_fn: &mut dyn FnMut(&str)) {
    let consumer = crate::config::agent_skarbiec_consumer();
    let token_file = crate::config::agent_skarbiec_token_file();
    if consumer.is_empty() || token_file.is_empty() || !Path::new(token_file).is_file() {
        return;
    }
    let at = now();
    let last = LAST_CHECK.load(Ordering::Relaxed);
    if last != 0 && at.saturating_sub(last) < CHECK_INTERVAL_SECONDS {
        return;
    }
    LAST_CHECK.store(at, Ordering::Relaxed);
    let Ok(vault) = crate::credential_store::owner::vault() else {
        return;
    };
    let Ok(launcher) = skarbiec_launcher() else {
        return;
    };
    let listing = match launcher_json(&launcher, &vault, &["grant", "list"]) {
        Ok(listing) => listing,
        Err(error) => {
            log_fn(&format!(
                "agent grant: cannot read the owner vault's grants for {consumer}: {error}"
            ));
            return;
        }
    };
    let record = grant_record(&listing, consumer);
    let expiry = record.as_ref().map(|(expires_at, _)| *expires_at);
    let due = match expiry {
        None => true,
        Some(expires_at) => expires_at.saturating_sub(at) < RENEWAL_WINDOW_SECONDS,
    };
    if !due {
        return;
    }
    // The grant keeps the capabilities it was issued with: the issuer chose
    // them, and a renewal that narrowed them to the field-level declaration
    // would silently drop what a release recipe reads. Only a grant the vault
    // has never held starts from the declaration.
    let issued = record
        .as_ref()
        .map(|(_, capabilities)| capabilities.clone())
        .unwrap_or_default();
    let capabilities = if issued.is_empty() {
        crate::config::agent_skarbiec_secret_fields()
            .iter()
            .map(|entry| format!("read:{entry}"))
            .collect::<Vec<_>>()
    } else {
        issued
    }
    .join(",");
    if capabilities.is_empty() {
        log_fn(&format!(
            "agent grant: {consumer} has no issued capabilities and declares no agent.skarbiec.secret_fields, so there is nothing to renew"
        ));
        return;
    }
    let state = match expiry {
        None => "is absent from the owner vault".to_string(),
        Some(expires_at) if expires_at <= at => format!("expired at {expires_at}"),
        Some(expires_at) => format!("ends at {expires_at}"),
    };
    let home = std::env::var("HOME").unwrap_or_default();
    let output = Command::new(&launcher)
        .args(["grant", "issue", consumer, "--capabilities", &capabilities])
        .args(["--replace-capabilities", "--token-file", token_file])
        .args(["--ttl-seconds", &GRANT_TTL_SECONDS.to_string()])
        .env("SKARBIEC_VAULT_FILE", &vault)
        .env("GNUPGHOME", format!("{home}/.gnupg"))
        .env(
            "PATH",
            "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        )
        .env_remove("SKARBIEC_UNLOCK")
        .env_remove("SKARBIEC_UNLOCK_FILE")
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let renewed = launcher_json(&launcher, &vault, &["grant", "list"])
                .ok()
                .and_then(|listing| grant_record(&listing, consumer))
                .map(|(expires_at, _)| expires_at);
            log_fn(&format!(
                "agent grant: {consumer} {state}; renewed for {GRANT_TTL_SECONDS} seconds, now ends at {}",
                renewed.map(|value| value.to_string()).unwrap_or_else(|| "an unread time".into())
            ));
        }
        Ok(output) => log_fn(&format!(
            "agent grant: {consumer} {state} and renewal failed: `{}` exited {}: {}",
            renewal_command(consumer, &capabilities, token_file),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(error) => log_fn(&format!(
            "agent grant: {consumer} {state} and renewal could not start: `{}`: {error}",
            renewal_command(consumer, &capabilities, token_file)
        )),
    }
}
