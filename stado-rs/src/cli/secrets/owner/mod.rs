//! `stado credentials vault move --to HOST --owner-uid UID [--apply] [--json]`:
//! the fleet vault, its owner identity and the Skarbiec service move to HOST.
//!
//! The vault's ciphertext moves with its placement profile, but the owner's
//! secret half does not: a moved vault no key on its new host opens is no
//! vault. So the owner changes first, to an identity whose secret half the
//! destination's keyring already holds, and the profile moves after:
//!
//! 1. the new owner is registered on the vault (its public key exported from
//!    the destination's keyring), unless the vault already registers it;
//! 2. the owner host rotates the vault to the new owner (`skarbiec
//!    rotate-owner`, every current and saved version, recovery kept);
//! 3. `stado placement move` carries the profile — the service, the vault and
//!    its journal — to the destination;
//! 4. the destination declares the vault as its own
//!    (`secrets.skarbiec.vault_file`) and the former owner host stops
//!    declaring one, so every credential verb reaches the moved vault;
//! 5. the destination's key-doctor reads the vault and its live items are
//!    exactly the ones the owner host held.
//!
//! A placement move that fails after the rotation gives the vault back to
//! its previous owner (`rollback`), so the owner host reads its vault again.
//! Once the move has carried the vault, it is not moved back: a later step
//! that fails (a declaration, the read on the destination) is refused naming
//! what stands then — the vault rotated to the new owner and on the
//! destination, which host declares the vault file, and whether key-doctor
//! read it — so the step can be finished from there. Without `--apply`
//! nothing changes: every reading the move depends on is made and reported.

mod plan;
mod remote;
mod rollback;

use serde_json::{json, Value};

use crate::cli::host::{remote_config_output, RemoteConfigAction};
use crate::cli::CmdError;
use crate::deploy::{CommandSpec, Runner};
use plan::Plan;
use remote::live_items;

/// The declaration a host's credential verbs read its own vault from.
const VAULT_FILE_KEY: &str = "secrets.skarbiec.vault_file";
/// The vault status key-doctor gives when a key on that host opens it.
const READABLE: &str = "readable";

pub(crate) async fn move_owner(
    to: &str,
    owner_uid: &str,
    apply: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let plan = Plan::read(to, owner_uid, &runner).await?;
    let mut report = plan.report();
    if apply {
        report["moved"] = apply_move(&plan, &runner).await?;
    }
    report["applied"] = Value::from(apply);
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!(
        "vault of {} ({} live items): owner {} on {} -> {} on {}{}",
        plan.profile.name,
        plan.live.len(),
        plan.previous.uid,
        plan.source.target.name,
        plan.owner_uid,
        plan.destination.target.name,
        if apply {
            ""
        } else {
            " (not applied; --apply moves it)"
        }
    );
    match &plan.missing_secret {
        None => println!(
            "{}'s keyring holds the new owner's secret half",
            plan.destination.target.name
        ),
        Some(cause) => println!(
            "{}'s keyring holds no secret half for {}: {cause}",
            plan.destination.target.name, plan.owner_uid
        ),
    }
    if plan.registered.is_none() {
        println!(
            "{} is not registered on the vault yet; the move registers it",
            plan.owner_uid
        );
    }
    Ok(())
}

/// Steps 1-5, each one's answer kept in the returned report.
async fn apply_move(plan: &Plan, runner: &Runner) -> Result<Value, CmdError> {
    if let Some(cause) = &plan.missing_secret {
        return Err(CmdError::refused(format!(
            "{}'s keyring holds no secret half for {}, so after the rotation no key there would open the vault: {cause}",
            plan.destination.target.name, plan.owner_uid
        )));
    }
    let registered = match &plan.registered {
        Some(recipient) => json!({ "already": recipient.fingerprint }),
        None => register(plan, runner).await.map_err(CmdError::refused)?,
    };
    let rotation = plan
        .source
        .skarbiec(&["rotate-owner", &plan.owner_uid], None, runner)
        .await
        .map_err(CmdError::refused)?;
    let moved = match placement_move(plan, runner).await {
        Ok(moved) => moved,
        Err(cause) => {
            let restored = match rollback::restore_owner(plan, runner).await {
                Ok(_) => format!(
                    "the vault was rotated back to {} and {} reads it again",
                    plan.previous.uid, plan.source.target.name
                ),
                Err(restore) => format!(
                    "rotating it back to {} failed too, so {} holds a vault only {} opens: {restore}",
                    plan.previous.uid, plan.source.target.name, plan.owner_uid
                ),
            };
            return Err(CmdError::refused(format!(
                "the placement move of {} to {} failed after the rotation: {cause}; {restored}",
                plan.profile.name, plan.destination.target.name
            )));
        }
    };
    let declared = format!("$HOME/{}", plan.relative);
    let stands = |step: &str, cause: String, destination_declares: bool, source_declares: bool| {
        CmdError::refused(format!(
            "{step} failed after the vault was rotated to {} and moved to {}: {cause}; \
             {} declares {VAULT_FILE_KEY} {}, {} {}; finish from here with the step that failed",
            plan.owner_uid,
            plan.destination.target.name,
            plan.destination.target.name,
            if destination_declares {
                "now"
            } else {
                "not yet"
            },
            plan.source.target.name,
            if source_declares {
                "still declares one it no longer serves"
            } else {
                "declares none"
            },
        ))
    };
    remote_config_output(
        &plan.destination.target,
        RemoteConfigAction::Set {
            key: VAULT_FILE_KEY,
            value: &declared,
        },
        runner,
    )
    .await
    .map_err(|error| {
        stands(
            "declaring the vault on the destination",
            error.to_string(),
            false,
            true,
        )
    })?;
    remote_config_output(
        &plan.source.target,
        RemoteConfigAction::Unset {
            key: VAULT_FILE_KEY,
        },
        runner,
    )
    .await
    .map_err(|error| {
        stands(
            "withdrawing the former owner host's declaration",
            error.to_string(),
            true,
            true,
        )
    })?;
    let verified = verify(plan, runner).await.map_err(|cause| {
        stands(
            "reading the moved vault on the destination",
            cause,
            true,
            false,
        )
    })?;
    Ok(json!({
        "registered": registered,
        "rotation": rotation,
        "placement": moved,
        "declared": { "host": plan.destination.target.name, "key": VAULT_FILE_KEY, "value": declared },
        "verified": verified,
    }))
}

/// Register the new owner on the vault with the public key the
/// destination's keyring exports for it.
async fn register(plan: &Plan, runner: &Runner) -> Result<Value, String> {
    let public = plan
        .destination
        .gpg(
            &["--armor", "--export", "--", &plan.owner_uid],
            None,
            runner,
        )
        .await?;
    if public.trim().is_empty() {
        return Err(format!(
            "{}'s keyring exported no public key for {}",
            plan.destination.target.name, plan.owner_uid
        ));
    }
    plan.source
        .skarbiec(
            &["add-user", &plan.owner_uid, "--import", "/dev/stdin"],
            Some(&public),
            runner,
        )
        .await
}

/// `stado placement move <profile's services> --to-host <destination>` by
/// this same Stado, through the runner that says what it waits for; its
/// committed report.
async fn placement_move(plan: &Plan, runner: &Runner) -> Result<Value, String> {
    let program = std::env::current_exe()
        .map_err(|error| format!("this Stado's own executable cannot be named: {error}"))?;
    let mut argv: Vec<String> = vec![
        program.display().to_string(),
        "placement".into(),
        "move".into(),
    ];
    argv.extend(plan.profile.services.iter().cloned());
    argv.extend([
        "--to-host".into(),
        plan.destination.target.name.clone(),
        "--json".into(),
    ]);
    let shown = argv.join(" ");
    let output = runner(CommandSpec { argv, stdin: None })
        .await
        .map_err(|error| format!("{shown} did not run: {error}"))?;
    if !output.succeeded() {
        return Err(format!("{shown} failed: {}", output.stderr.trim()));
    }
    serde_json::from_str(output.stdout.trim())
        .map_err(|error| format!("{shown} printed no readable report: {error}"))
}

/// The destination opens the moved vault and holds every live item the
/// owner host held, no more and no fewer.
async fn verify(plan: &Plan, runner: &Runner) -> Result<Value, String> {
    let doctor = plan
        .destination
        .skarbiec(&["key-doctor"], None, runner)
        .await?;
    if doctor.get("status").and_then(Value::as_str) != Some(READABLE) {
        return Err(format!(
            "{}'s key-doctor does not read the moved vault: {doctor}",
            plan.destination.target.name
        ));
    }
    let listed = plan.destination.skarbiec(&["list"], None, runner).await?;
    let live = live_items(&listed)?;
    if live != plan.live {
        let missing: Vec<&String> = plan.live.iter().filter(|id| !live.contains(id)).collect();
        let extra: Vec<&String> = live.iter().filter(|id| !plan.live.contains(id)).collect();
        return Err(format!(
            "{} holds other live items than the vault it moved from: missing {missing:?}, extra {extra:?}",
            plan.destination.target.name
        ));
    }
    Ok(json!({ "key_doctor": doctor, "live_items": live.len() }))
}
