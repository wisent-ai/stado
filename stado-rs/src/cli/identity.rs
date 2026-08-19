//! Which host holds which identity, and whether that is still true.
//!
//! A compute target is chosen for what it can do. A few kinds of work instead need
//! the one machine that *is* something: the Mac an Apple account is signed into, the
//! host a hardware token is plugged into, the box a licence is bound to. That is not
//! capacity and not permission, so `weles.actions` cannot express it.
//!
//! Two commands, matching the two questions an operator actually asks:
//!
//!   list    what does the registry claim, and has any host confirmed it
//!   verify  go and look, then say what is true right now
//!
//! `verify` reads the host rather than trusting the declaration, because these
//! identities are granted elsewhere and revoked without notice: an Apple account
//! signs out on a password change, and nothing tells the fleet. A declaration that
//! is never re-checked is the failure mode this module exists to remove -- the flow
//! would otherwise dispatch to a host that stopped qualifying weeks ago and fail
//! deep inside a browser trajectory with a timeout.

use anyhow::Result;
use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::targets::{load_registry_auto, ComputeTarget, IdentityBinding};

const APPLE_ACCOUNT: &str = "apple-account";

/// The per-user Apple-account probe, embedded in this binary and run as one
/// fixed remote script by [`observe_user_apple_accounts`]. Kept as a checked-in
/// file rather than a string literal so it is reviewed and read as the shell
/// program it is.
const APPLE_ACCOUNT_PROBE: &str = include_str!("../../scripts/probe-apple-account-holders.sh");

/// Read a host's live Apple-account bindings through Stado's own approved channel.
///
/// Not `ssh`. A one-liner over ssh is the same action with the audit trail removed,
/// and this file previously did exactly that -- teaching the anti-pattern from inside
/// the tool meant to replace it. `host exec` runs one fixed, read-only, allowlisted
/// argv, so the probe cannot be pointed at a path and cannot grow into a shell.
///
/// The reading is the login user's own, because `defaults read` carries no path and
/// no sudo. A binding naming some other user on that machine is therefore reported
/// unknown rather than guessed at: an account signed into `charles` says nothing
/// about whether `weles-apple` can display a prompt.
///
/// `None` means the probe could not run -- unreachable host, refused channel, no such
/// domain. That is unknown, never absent, because sending an operator to re-enroll a
/// machine that is actually signed in is the worse error.
fn account_ids(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| line.contains("AccountID"))
        .filter_map(|line| {
            let mut quoted = line.split('"');
            quoted.next();
            quoted.next().map(str::to_string)
        })
        .collect()
}

async fn observe_apple_accounts(target_name: &str) -> Option<Vec<String>> {
    let runner = crate::deploy::production_runner();
    let words = vec![
        "defaults".to_string(),
        "read".to_string(),
        "MobileMeAccounts".to_string(),
    ];
    let report = crate::deploy::host_exec::exec_host(target_name, &words, &runner)
        .await
        .ok()?;
    if report.get("status").and_then(Value::as_str) != Some(crate::deploy::host_exec::OK_STATUS) {
        return None;
    }
    let found = account_ids(report.get("stdout").and_then(Value::as_str)?);
    if found.is_empty() {
        None
    } else {
        Some(found)
    }
}

/// Ask the host which of its users hold Apple accounts, via the probe embedded
/// in this binary.
///
/// `defaults read` answers only for the user the channel logs in as, so a binding
/// naming anyone else could never be anything but `unknown`. That word covered two
/// opposite situations -- the user is not signed in, and this probe was not allowed
/// to look -- and an operator acts differently on each. The probe reports them
/// apart: an account list, `none`, or `unreadable`.
///
/// The probe travels with stado and runs as one fixed read-only remote script --
/// the channel the retired helper pair used to be the long way around -- so a host
/// that cannot answer is one the channel itself could not reach, and the answer
/// stays `unknown`, which is the honest answer when nothing on that host can
/// produce a better one.
async fn observe_user_apple_accounts(target_name: &str, user: &str) -> Option<Vec<String>> {
    let runner = crate::deploy::production_runner();
    let report =
        crate::deploy::host_channel::run_fixed_script(target_name, APPLE_ACCOUNT_PROBE, &runner)
            .await
            .ok()?;
    for line in report.lines() {
        let mut columns = line.split('\t');
        let named = columns.next()?;
        let accounts = columns.next().unwrap_or_default().trim();
        if named != user {
            continue;
        }
        // `unreadable` is the probe declining to answer, which must stay unknown
        // rather than becoming "not signed in".
        if accounts == "unreadable" {
            return None;
        }
        if accounts == "none" {
            return Some(Vec::new());
        }
        return Some(accounts.split_whitespace().map(str::to_string).collect());
    }
    None
}

/// Does the approved channel land on the very user this binding names?
///
/// The channel logs in as the `user` half of the target's ssh destination and reads
/// that account's own preferences. When the binding names somebody else, the probe is
/// looking at the wrong desk, and a binding that names nobody is taken at the login
/// user, which is who the channel is.
fn probes_own_user(target: &ComputeTarget, binding: &IdentityBinding) -> bool {
    let Some(declared) = binding.user.as_deref() else {
        return true;
    };
    target
        .ssh
        .as_deref()
        .and_then(|destination| destination.split('@').next())
        .is_some_and(|login| login == declared)
}

/// Is this registry target the machine we are running on?
///
/// Matched on the short hostname and the declared hostnames, because a registry name
/// is an operator label ("operator-host") and need not equal what the OS reports.
fn is_local_target(target: &ComputeTarget) -> bool {
    let Ok(output) = std::process::Command::new("hostname").arg("-s").output() else {
        return false;
    };
    let host = String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_lowercase();
    if host.is_empty() {
        return false;
    }
    // Compare the leading label only. `hostname -s` gives "Lukaszs-MacBook-Pro-5485"
    // while the registry records the mDNS form "operator-host.local", and
    // an exact match silently fails on that suffix -- reporting the local machine as
    // unverifiable while standing on it.
    let label = |value: &str| {
        value
            .to_lowercase()
            .split('.')
            .next()
            .unwrap_or("")
            .to_string()
    };
    let host = label(&host);
    label(&target.name) == host || target.hostnames.iter().any(|name| label(name) == host)
}

/// The Apple accounts the current user is signed into on this machine.
fn local_apple_accounts() -> Option<Vec<String>> {
    let output = std::process::Command::new("defaults")
        .args(["read", "MobileMeAccounts"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let found = account_ids(&String::from_utf8_lossy(&output.stdout));
    if found.is_empty() {
        None
    } else {
        Some(found)
    }
}

fn binding_row(target: &ComputeTarget, binding: &IdentityBinding, observed: Option<bool>) -> Value {
    json!({
        // The registry's name for the machine, and deliberately not an address.
        // Callers route work to the holder through the registry channel, which
        // resolves the target itself; handing them a destination as well would be a
        // second way to say where a host is, and the one that goes stale -- which is
        // exactly how the hand-written APPLE_2FA_MAC_* variables came to exist.
        "host": target.name,
        "kind": binding.kind,
        "identity": binding.identity,
        "user": binding.user,
        "declared": true,
        "observed": observed,
        "verified_at": binding.verified_at,
    })
}

pub async fn list(json_output: bool) -> Result<(), CmdError> {
    let registry = load_registry_auto()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let rows: Vec<Value> = registry
        .targets
        .iter()
        .flat_map(|target| {
            target
                .identities
                .iter()
                .map(move |binding| binding_row(target, binding, None))
        })
        .collect();
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).unwrap_or_default()
        );
        return Ok(());
    }
    if rows.is_empty() {
        println!("no host declares an identity binding");
        return Ok(());
    }
    println!("{:<24} {:<16} {:<32} USER", "HOST", "KIND", "IDENTITY");
    for row in &rows {
        println!(
            "{:<24} {:<16} {:<32} {}",
            row["host"].as_str().unwrap_or("-"),
            row["kind"].as_str().unwrap_or("-"),
            row["identity"].as_str().unwrap_or("-"),
            row["user"].as_str().unwrap_or("-"),
        );
    }
    Ok(())
}

/// Resolve which host currently holds an identity, checking rather than trusting.
///
/// Exits non-zero when nothing satisfies the binding. That is the point: a caller
/// that needs a trusted device can gate on this and fail with "no host holds
/// <identity>" instead of dispatching work that cannot possibly complete.
pub async fn verify(kind: String, identity: String, json_output: bool) -> Result<(), CmdError> {
    let registry = load_registry_auto()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut rows: Vec<Value> = Vec::new();
    let mut satisfied = false;

    for target in &registry.targets {
        for binding in &target.identities {
            if binding.kind != kind || binding.identity != identity {
                continue;
            }
            let observed = match binding.kind.as_str() {
                // Reading the machine we are already running on needs neither SSH nor
                // sudo only when the binding names the channel's login user. A binding
                // for another local user still needs the installed multi-user probe;
                // reading this process's preferences would confidently answer the
                // wrong account.
                APPLE_ACCOUNT if is_local_target(target) && probes_own_user(target, binding) => {
                    local_apple_accounts().map(|found| found.iter().any(|e| e == &identity))
                }
                // A binding naming someone other than the channel's login user is
                // beyond `defaults read`, which carries no path and no sudo. The
                // installed probe reads each user's own preference file and reports
                // an answer apart from a refusal, so this is unknown only when that
                // probe is absent or declined -- not merely because the binding names
                // a second user. Saying `false` without one of those readings would
                // be a false negative dressed as a measurement.
                APPLE_ACCOUNT if !probes_own_user(target, binding) => {
                    let user = binding.user.as_deref().unwrap_or_default();
                    observe_user_apple_accounts(&target.name, user)
                        .await
                        .map(|found| found.iter().any(|entry| entry == &identity))
                }
                APPLE_ACCOUNT => observe_apple_accounts(&target.name)
                    .await
                    .map(|found| found.iter().any(|entry| entry == &identity)),
                // An identity family we cannot probe stays unknown rather than being
                // reported as present: claiming verification we did not perform is
                // worse than admitting the gap.
                _ => None,
            };
            satisfied |= observed == Some(true);
            rows.push(binding_row(target, binding, observed));
        }
    }

    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "kind": kind,
                "identity": identity,
                "satisfied": satisfied,
                "bindings": rows,
            }))
            .unwrap_or_default()
        );
    } else if rows.is_empty() {
        println!("no host declares {kind} {identity}");
    } else {
        for row in &rows {
            let observed = match row["observed"].as_bool() {
                Some(true) => "held",
                Some(false) => "MISSING",
                None => "unknown",
            };
            println!(
                "{:<24} {:<32} {}",
                row["host"].as_str().unwrap_or("-"),
                row["identity"].as_str().unwrap_or("-"),
                observed
            );
        }
    }

    if satisfied {
        Ok(())
    } else {
        Err(CmdError::click(format!(
            "no host holds {kind} {identity}; enroll one before dispatching work that needs it"
        )))
    }
}
