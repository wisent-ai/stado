//! Which host holds which identity, and whether that is still true.
//!
//! A compute target is chosen for what it can do. A few kinds of work instead need
//! the one machine that *is* something: the Mac an Apple account is signed into, the
//! host a hardware token is plugged into, the box a licence is bound to. That is not
//! capacity and not permission, so `weles.actions` cannot express it.
//!
//! Three commands, matching the questions an operator and a trajectory ask:
//!
//!   list                   what does the registry claim
//!   verify                 what does each host confirm right now
//!   relay-apple-challenge  capture on the holder and store on the worker
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
use crate::targets::{load_registry_auto, ComputeTarget, IdentityBinding, Registry};

const APPLE_ACCOUNT: &str = "apple-account";

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

/// Ask the host which of its users hold Apple accounts, read natively over the
/// host channel: one directory listing, two file tests and one `plutil -p`,
/// with every branch taken here.
///
/// `defaults read` answers only for the user the channel logs in as, so a binding
/// naming anyone else could never be anything but `unknown`. That word covered two
/// opposite situations -- the user is not signed in, and this probe was not allowed
/// to look -- and an operator acts differently on each. The probe reports them
/// apart: an account list, `none`, or `unreadable`.
///
/// The probe rides Stado's own audited channel -- the one the retired helper
/// pair used to be the long way around -- so a host that cannot answer is one
/// the channel itself could not reach, and the answer stays `unknown`, which
/// is the honest answer when nothing on that host can produce a better one.
async fn observe_user_apple_accounts(target_name: &str, user: &str) -> Option<Vec<String>> {
    let runner = crate::deploy::production_runner();
    let target = crate::deploy::host_channel::canonical_target(target_name)
        .await
        .ok()?;

    // The probe's per-user walk, narrowed to the one user the binding names:
    // `ls /Users` decides which homes the host has, and `Shared`, `Guest` and
    // dot-directories are not users. A user with no home directory on that
    // host is a user the probe could not look at -- unknown, not absent.
    let homes = crate::deploy::host_channel::run_program(&target, &["/bin/ls", "/Users"], &runner)
        .await
        .ok()?;
    if !homes.ok()
        || user == "Shared"
        || user == "Guest"
        || user.starts_with('.')
        || !homes.stdout.lines().any(|name| name == user)
    {
        return None;
    }

    // Read the preference file directly: the account identifiers it holds,
    // `unreadable` when the channel may not open it, or `none` when there is
    // no such file. Only the first and the last are observations; the middle
    // one is the probe admitting its limit, which is the distinction the whole
    // thing exists to make. Read-only throughout: a preference file is opened
    // and nothing is written anywhere.
    //
    // The order below is the correction. `test -f` inside another user's home
    // fails on macOS for lack of search permission — homes are 700 — and this
    // returned that as `Some(vec![])`, which reads as "that user is not signed
    // in". On 2026-09-04 it said exactly that about an account the operator
    // had been signed into on that Mac for weeks, and the Developer ID run
    // refused with `no host holds apple-account`. The `-r` test meant to catch
    // it sat BEHIND the `-f` test, so it could never fire. Absence is only
    // claimed once the directory has been shown to be searchable.
    let plist = format!("/Users/{user}/Library/Preferences/MobileMeAccounts.plist");
    let quoted = crate::deploy::shlex_quote(&plist);
    let directory = crate::deploy::shlex_quote(&format!("/Users/{user}/Library/Preferences"));
    let readable =
        crate::deploy::host_channel::remote_test(&target, &format!("-r {quoted}"), &runner)
            .await
            .ok()?;
    if readable {
        let printed = crate::deploy::host_channel::run_program(
            &target,
            &["/usr/bin/plutil", "-p", &plist],
            &runner,
        )
        .await
        .ok()?;
        return Some(plutil_account_ids(&printed.stdout));
    }
    // Not readable as the channel's user. The fleet already reaches root on
    // these hosts for `launchctl` and `install`, so the same grant answers
    // this question rather than leaving it to a guess; a host that does not
    // grant it stays unknown.
    let privileged = crate::deploy::host_channel::run_program(
        &target,
        &["/usr/bin/sudo", "-n", "/usr/bin/plutil", "-p", &plist],
        &runner,
    )
    .await
    .ok()?;
    if privileged.ok() {
        return Some(plutil_account_ids(&privileged.stdout));
    }
    // Neither read worked. Absence is a claim, and it is only made when the
    // channel could look at the directory and found no file there.
    let searchable =
        crate::deploy::host_channel::remote_test(&target, &format!("-x {directory}"), &runner)
            .await
            .ok()?;
    let present =
        crate::deploy::host_channel::remote_test(&target, &format!("-f {quoted}"), &runner)
            .await
            .ok()?;
    if searchable && !present {
        return Some(Vec::new());
    }
    None
}

/// Every `AccountID` a `plutil -p` dump names, in the order printed.
///
/// A separate parser from [`account_ids`] on purpose: `defaults read` writes
/// `AccountID = "x";` and `plutil -p` writes `"AccountID" => "x"`, so one
/// quoted-segment index cannot read both.
fn plutil_account_ids(printed: &str) -> Vec<String> {
    printed
        .lines()
        .filter(|line| line.contains("AccountID"))
        .filter_map(|line| line.split('"').nth(3).map(str::to_string))
        .collect()
}

/// Does the approved channel land on the very user this binding names?
///
/// Every declared connection must log in as the binding's user. A fallback
/// that lands on another account would make the observation depend on which
/// network happened to answer first.
fn probes_own_user(target: &ComputeTarget, binding: &IdentityBinding) -> bool {
    let Some(declared) = binding.user.as_deref() else {
        return true;
    };
    target.has_ssh_connection()
        && target.ssh_connections().all(|(_, destination)| {
            destination
                .split_once('@')
                .map(|(login, _)| login == declared)
                .unwrap_or(false)
        })
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

/// Can the fleet act inside the session this binding names?
///
/// A per-user identity is only usable where its own session can be driven: a two-factor
/// notification for an Apple account is delivered into the session of the user signed
/// into it, and no other session on that Mac can read or answer it.
///
/// The full GUI verdict matters. Merely matching the console user used to return true
/// while Accessibility was not granted and the CuaDriver runtime was absent. That is
/// not a drivable session; it is a correctly named session with no working actuator.
async fn drivable_session(
    kind: &str,
    target: &ComputeTarget,
    binding: &IdentityBinding,
) -> Option<bool> {
    let declared = binding.user.as_deref()?;
    let password = super::service::host_sudo_password(target).await.ok()?;
    let runner = crate::deploy::production_runner();
    if kind == APPLE_ACCOUNT {
        crate::deploy::host_gui_automation::apple_challenge_session_ready_for(
            target,
            declared,
            password.as_deref(),
            &runner,
        )
        .await
        .ok()
    } else {
        crate::deploy::host_gui_automation::automated_session_ready_for(
            target,
            declared,
            password.as_deref(),
            &runner,
        )
        .await
        .ok()
    }
}

fn binding_row(
    target: &ComputeTarget,
    binding: &IdentityBinding,
    observed: Option<bool>,
    drivable: Option<bool>,
) -> Value {
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
        // Whether the fleet can act in this binding's session. `null` when the host
        // could not be asked, and never conflated with `false`.
        "drivable_session": drivable,
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
                // `list` prints the declaration alone and reaches no host, so both
                // measured columns are absent here rather than guessed.
                .map(move |binding| binding_row(target, binding, None, None))
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

struct Verification {
    rows: Vec<Value>,
    satisfied: bool,
}

async fn verified_bindings(registry: &Registry, kind: &str, identity: &str) -> Verification {
    let mut rows = Vec::new();
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
                    local_apple_accounts().map(|found| found.iter().any(|entry| entry == identity))
                }
                // The installed probe reads a named user's own preferences. Unknown
                // means the host could not be asked, never that the account is absent.
                APPLE_ACCOUNT if !probes_own_user(target, binding) => {
                    let user = binding.user.as_deref().unwrap_or_default();
                    observe_user_apple_accounts(&target.name, user)
                        .await
                        .map(|found| found.iter().any(|entry| entry == identity))
                }
                APPLE_ACCOUNT => observe_apple_accounts(&target.name)
                    .await
                    .map(|found| found.iter().any(|entry| entry == identity)),
                // An identity family we cannot probe stays unknown rather than being
                // reported as present.
                _ => None,
            };
            satisfied |= observed == Some(true);
            let drivable = drivable_session(kind, target, binding).await;
            rows.push(binding_row(target, binding, observed, drivable));
        }
    }
    Verification { rows, satisfied }
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
    let Verification { rows, satisfied } = verified_bindings(&registry, &kind, &identity).await;

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
            // Two separate questions, so two separate words: whether the identity is
            // there, and whether the fleet can act where it is. False covers either a
            // different session or an incomplete GUI runtime; both refuse placement.
            let session = match row["drivable_session"].as_bool() {
                Some(true) => "drivable",
                Some(false) => "NOT-DRIVABLE",
                None => "unknown",
            };
            println!(
                "{:<24} {:<32} {:<8} {}",
                row["host"].as_str().unwrap_or("-"),
                row["identity"].as_str().unwrap_or("-"),
                observed,
                session
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

/// Capture a trusted-device code on the verified holder and put it into the
/// Weles capability broker on the host executing this command.
pub async fn relay_apple_challenge(
    identity: String,
    authorization_id: String,
    preflight: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    if identity.trim().is_empty() {
        return Err(CmdError::click("an Apple account identity is required"));
    }
    if uuid::Uuid::parse_str(&authorization_id).is_err() {
        return Err(CmdError::click("--authorization-id must be a UUID"));
    }
    let registry = load_registry_auto()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let Verification { rows, .. } = verified_bindings(&registry, APPLE_ACCOUNT, &identity).await;
    let holder = rows.iter().find(|row| {
        row.get("observed").and_then(Value::as_bool) == Some(true)
            && row.get("drivable_session").and_then(Value::as_bool) == Some(true)
    });
    let Some(holder) = holder else {
        let observed = rows
            .iter()
            .filter(|row| row.get("observed").and_then(Value::as_bool) == Some(true))
            .filter_map(|row| {
                let host = row.get("host")?.as_str()?;
                let user = row
                    .get("user")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown-user");
                Some(format!("{host}/{user}"))
            })
            .collect::<Vec<_>>();
        if observed.is_empty() {
            return Err(CmdError::click(format!(
                "no verified host holds apple-account {identity}"
            )));
        }
        return Err(CmdError::click(format!(
            "apple-account {identity} is held on {}, but none of those Apple challenge sessions is drivable",
            observed.join(", ")
        )));
    };
    let holder_name = holder
        .get("host")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::click("the Apple identity report names no holder"))?;
    let holder_user = holder
        .get("user")
        .and_then(Value::as_str)
        .ok_or_else(|| CmdError::click("the Apple identity holder names no macOS user"))?;
    let holder_target = registry
        .targets
        .iter()
        .find(|target| target.name == holder_name)
        .ok_or_else(|| CmdError::click("the Apple identity holder left the registry"))?;

    let destinations = registry
        .targets
        .iter()
        .filter(|target| crate::deploy::host_channel::target_is_this_host(target))
        .collect::<Vec<_>>();
    let [destination] = destinations.as_slice() else {
        return Err(CmdError::click(format!(
            "the current machine resolves to {} registry targets; exactly one is required",
            destinations.len()
        )));
    };
    let password = super::service::host_sudo_password(holder_target).await?;
    let runner = crate::deploy::production_runner();
    let resource = format!("challenge:apple/{authorization_id}");
    let broker = crate::deploy::host_capability::resolve(
        destination,
        &crate::deploy::weles_browser_task::weles_api_broker_files(),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.0))?;
    if preflight {
        crate::deploy::host_gui_automation::preflight_apple_challenge(
            holder_target,
            holder_user,
            password.as_deref(),
            &runner,
        )
        .await
        .map_err(|error| CmdError::click(error.0))?;
        let receipt = json!({
            "status": "ready",
            "identity": identity,
            "holder": holder_name,
            "user": holder_user,
            "destination": destination.name,
            "resource": resource,
        });
        if json_output {
            println!(
                "{}",
                serde_json::to_string_pretty(&receipt).unwrap_or_default()
            );
        } else {
            println!(
                "Apple challenge relay is ready from {holder_name}/{holder_user} to {}",
                destination.name
            );
        }
        return Ok(());
    }

    let mut code = crate::deploy::host_gui_automation::capture_apple_challenge(
        holder_target,
        holder_user,
        &authorization_id,
        90,
        password.as_deref(),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.0))?;
    let stored = crate::deploy::host_capability::apple_challenge_put(
        destination,
        &broker,
        &resource,
        &code,
        &runner,
    )
    .await;
    code.clear();
    stored.map_err(|error| CmdError::click(error.0))?;

    let receipt = json!({
        "status": "stored",
        "identity": identity,
        "holder": holder_name,
        "user": holder_user,
        "destination": destination.name,
        "resource": resource,
    });
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&receipt).unwrap_or_default()
        );
    } else {
        println!(
            "stored Apple challenge from {holder_name}/{holder_user} for {}",
            destination.name
        );
    }
    Ok(())
}

/// Issue the three authorization-bound Apple login capabilities in the broker
/// on the worker that will redeem them.
pub async fn issue_apple_capabilities(
    target_name: String,
    agent: String,
    authorization_id: String,
    ttl_seconds: u64,
    json_output: bool,
) -> Result<(), CmdError> {
    if uuid::Uuid::parse_str(&authorization_id).is_err() {
        return Err(CmdError::click("--authorization-id must be a UUID"));
    }
    if agent.trim().is_empty() || agent.trim() != agent {
        return Err(CmdError::click("--agent must be a non-empty exact name"));
    }
    if !(60..=3600).contains(&ttl_seconds) {
        return Err(CmdError::click("--ttl-seconds must be between 60 and 3600"));
    }
    let registry = load_registry_auto()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let target = registry
        .targets
        .iter()
        .find(|target| target.name == target_name)
        .ok_or_else(|| CmdError::click(format!("unknown target {target_name}")))?;
    let runner = crate::deploy::production_runner();
    let broker = crate::deploy::host_capability::resolve(
        target,
        &crate::deploy::weles_browser_task::weles_api_broker_files(),
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.0))?;
    let ttl = ttl_seconds.to_string();

    let email_purpose = "weles.browser.fill";
    let email_resource = "origin:https://idmsa.apple.com/email";
    let email_id = crate::deploy::host_capability::issue(
        target,
        &broker,
        &crate::deploy::host_capability::Issuance {
            agent: &agent,
            purpose: email_purpose,
            resource: email_resource,
            capability_target: "weles",
            ttl_seconds: &ttl,
            max_uses: "1",
            authorization_id: Some(&authorization_id),
        },
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.0))?;
    let password_purpose = "weles.browser.fill";
    let password_resource = "origin:https://idmsa.apple.com/password";
    let password_id = crate::deploy::host_capability::issue(
        target,
        &broker,
        &crate::deploy::host_capability::Issuance {
            agent: &agent,
            purpose: password_purpose,
            resource: password_resource,
            capability_target: "weles",
            ttl_seconds: &ttl,
            max_uses: "1",
            authorization_id: Some(&authorization_id),
        },
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.0))?;
    let challenge_purpose = "weles.apple.2fa";
    let challenge_resource = format!("challenge:apple/{authorization_id}");
    let challenge_id = crate::deploy::host_capability::issue(
        target,
        &broker,
        &crate::deploy::host_capability::Issuance {
            agent: &agent,
            purpose: challenge_purpose,
            resource: &challenge_resource,
            capability_target: "weles",
            ttl_seconds: &ttl,
            max_uses: "1",
            authorization_id: Some(&authorization_id),
        },
        &runner,
    )
    .await
    .map_err(|error| CmdError::click(error.0))?;

    let capability_ref = |capability_id: String, purpose: &str, resource: &str| {
        json!({
            "capability_id": capability_id,
            "purpose": purpose,
            "resource": resource,
            "target": "weles",
            "authorization_id": authorization_id,
        })
    };
    let receipt = json!({
        "status": "issued",
        "target": target.name,
        "authorization_id": authorization_id,
        "capabilities": {
            "email": capability_ref(email_id, email_purpose, email_resource),
            "password": capability_ref(password_id, password_purpose, password_resource),
            "two_factor": {
                "mode": "capability",
                "capability": capability_ref(
                    challenge_id,
                    challenge_purpose,
                    &challenge_resource
                ),
            },
        },
    });
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&receipt).unwrap_or_default()
        );
    } else {
        println!(
            "issued one Apple login authorization on {} for {agent}",
            target.name
        );
    }
    Ok(())
}
