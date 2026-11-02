//! A vault move that stopped part way, taken up again.
//!
//! When the placement move failed after the rotation, the vault goes back to
//! its previous owner ([`restore_owner`]): the destination rotates a copy back
//! with the previous owner's public key from the owner host's keyring, the
//! rotated copy replaces the owner host's vault, and the owner host's own
//! `keys check` must read it, or the restoration is reported as failed.
//!
//! When the placement committed and the declarations did not follow (the
//! caller ended, or could not read the committed report), the directory places
//! the vault on the destination while the hosts still declare it where it
//! was. `credentials vault move --to <that host>` then finishes the move
//! ([`finish`]) instead of refusing it as already there: the host the
//! directory places it on declares the vault the profile carries, every other
//! host of the profile stops declaring one, and that host's `keys check` and
//! owner are read.

use serde_json::{json, Value};

use super::plan::{owner_of, Plan};
use super::remote::{live_items, VaultHost};
use crate::cli::host::{remote_config_output, RemoteConfigAction};
use crate::cli::CmdError;
use crate::deploy::Runner;
use crate::placement::{PlacementProfile, StateRoot};
use crate::targets::ComputeTarget;

/// Restore the previous owner on the owner host; `Ok` carries `keys check`'s
/// answer there, `Err` the step that failed and why.
pub(super) async fn restore_owner(plan: &Plan, runner: &Runner) -> Result<Value, String> {
    let exported = plan
        .source
        .skarbiec(&["recipient", "export", &plan.previous.uid], None, runner)
        .await?;
    let public = exported
        .get("public_key")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            format!(
                "{}: recipient export for {} returned no public_key",
                plan.source.target.name, plan.previous.uid
            )
        })?;
    if public.trim().is_empty() {
        return Err(format!(
            "{}'s keyring exported no public key for the previous owner {}",
            plan.source.target.name, plan.previous.uid
        ));
    }
    plan.destination
        .gpg(&["--import"], Some(public), runner)
        .await?;
    let rotated = plan.source.read_file(&plan.source.vault, runner).await?;
    let staged = format!("{}.restoring", plan.destination.vault);
    plan.destination
        .write_file(&staged, &rotated, runner)
        .await?;
    let restorer = plan.destination.with_vault(staged.clone());
    let rotation = restorer
        .skarbiec(&["owner", "rotate", &plan.previous.uid], None, runner)
        .await;
    let restored = match rotation {
        Ok(_) => plan.destination.read_file(&staged, runner).await,
        Err(cause) => Err(cause),
    };
    let removed = plan.destination.remove_file(&staged, runner).await;
    let restored = restored?;
    removed?;
    plan.source
        .write_file(&plan.source.vault, &restored, runner)
        .await?;
    readable(&plan.source, "the restored vault", runner).await
}

/// The vault as the directory places it, read before anything changes: the
/// host it is placed on, the vault file the profile carries, and the vault
/// file each host of the profile declares.
pub(super) struct Placed {
    host: VaultHost,
    relative: String,
    declared: Vec<(ComputeTarget, Option<String>)>,
}

impl Placed {
    pub(super) async fn read(
        target: ComputeTarget,
        profile: &PlacementProfile,
        document: &Value,
        runner: &Runner,
    ) -> Result<Self, CmdError> {
        let relative = carried_vault(profile)?;
        let host = VaultHost::destination(target, &relative, document, runner).await?;
        let registry =
            crate::targets::load_registry_from_value(document).map_err(CmdError::from)?;
        let mut declared = Vec::new();
        for name in profile.hosts.keys() {
            let target = crate::cli::resolved_host(&registry, name)?.clone();
            let file = declared_vault(&target, runner).await?;
            declared.push((target, file));
        }
        Ok(Self { host, relative, declared })
    }
}

/// Finish a move whose placement committed: refused when the vault on the
/// host the directory places it on is not owned by `owner_uid`, or when that
/// host declares a vault file other than the one the profile carries.
pub(super) async fn finish(
    placed: &Placed,
    owner_uid: &str,
    apply: bool,
    json_output: bool,
    runner: &Runner,
) -> Result<(), CmdError> {
    let host = &placed.host;
    let name = &host.target.name;
    let recipients = host.skarbiec(&["recipient", "list"], None, runner).await.map_err(CmdError::refused)?;
    let owner = owner_of(&recipients)?;
    if owner.uid != owner_uid {
        return Err(CmdError::refused(format!(
            "the directory places the fleet vault on {name}, whose vault is owned by {}, not \
             {owner_uid}; the move to {name} is done, and moving its owner is `stado credentials \
             vault move` to another host",
            owner.uid
        )));
    }
    let wanted = format!("$HOME/{}", placed.relative);
    let mut steps = Vec::new();
    for (target, declared) in &placed.declared {
        let action = match (target.name == *name, declared) {
            (true, Some(file)) if *file == host.vault => "keep",
            (true, Some(file)) => {
                return Err(CmdError::refused(format!(
                    "{name} declares {} {file}, not {}, the vault the placement profile carries; \
                     which one is the fleet vault cannot be told",
                    super::VAULT_FILE_KEY, host.vault
                )))
            }
            (true, None) => "declare",
            (false, Some(_)) => "withdraw",
            (false, None) => continue,
        };
        steps.push((target, declared.clone(), action));
    }
    // The host that serves the vault declares it before any other stops: a
    // credential verb reads a declared vault at every moment between them.
    steps.sort_by_key(|(_, _, action)| *action == "withdraw");
    let mut declarations = Vec::new();
    for (target, declared, action) in steps {
        if apply && action != "keep" {
            let change = match action {
                "declare" => RemoteConfigAction::Set { key: super::VAULT_FILE_KEY, value: &wanted },
                _ => RemoteConfigAction::Unset { key: super::VAULT_FILE_KEY },
            };
            remote_config_output(target, change, runner).await.map_err(|error| {
                CmdError::refused(format!(
                    "finishing the move of the fleet vault to {name}: {action} {} on {} failed: \
                     {error}; the declarations before it were made, so run the command again",
                    super::VAULT_FILE_KEY, target.name
                ))
            })?;
        }
        declarations.push(json!({ "host": target.name, "declared": declared, "action": action }));
    }
    let key_doctor = readable(host, "the vault", runner).await.map_err(CmdError::refused)?;
    let listed = host.skarbiec(&["list"], None, runner).await.map_err(CmdError::refused)?;
    let live = live_items(&listed).map_err(CmdError::declaration)?.len();
    let report = json!({
        "placed_on": name, "vault": host.vault, "owner_uid": owner.uid, "declare": wanted,
        "declarations": declarations, "key_doctor": key_doctor, "live_items": live, "applied": apply,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    println!("the directory places the fleet vault on {name} ({live} live items, owner {owner_uid})");
    for line in report["declarations"].as_array().into_iter().flatten() {
        println!(
            "  {}: {} {}{}",
            line["host"].as_str().unwrap_or_default(),
            line["action"].as_str().unwrap_or_default(),
            super::VAULT_FILE_KEY,
            if apply { "" } else { " (not applied; --apply does it)" }
        );
    }
    Ok(())
}

/// The one file the profile carries as required home state: the vault.
fn carried_vault(profile: &PlacementProfile) -> Result<String, CmdError> {
    let required: Vec<&str> = profile
        .state
        .iter()
        .filter(|state| state.required && state.root == StateRoot::Home)
        .map(|state| state.path.as_str())
        .collect();
    match required.as_slice() {
        [vault] => Ok((*vault).to_string()),
        _ => Err(CmdError::declaration(format!(
            "placement profile {} carries {} required home state(s) ({}), so which one is the \
             fleet vault cannot be told",
            profile.name,
            required.len(),
            required.join(", ")
        ))),
    }
}

/// The vault file `target` declares, as its own Stado resolves it under its
/// home; `None` when it declares none.
async fn declared_vault(target: &ComputeTarget, runner: &Runner) -> Result<Option<String>, CmdError> {
    let shown = remote_config_output(target, RemoteConfigAction::Show, runner).await?;
    let document: Value = serde_json::from_str(&shown).map_err(|error| {
        CmdError::click(format!("{}: its Stado configuration is not JSON: {error}", target.name))
    })?;
    let Some(declared) = document
        .pointer("/resolved/skarbiec_vault_file")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    Ok(Some(match declared.strip_prefix("$HOME/") {
        Some(tail) => {
            let home = crate::deploy::host_channel::remote_home(target, runner)
                .await
                .map_err(CmdError::from)?;
            format!("{home}/{tail}")
        }
        None => declared.to_string(),
    }))
}

/// `keys check` on `host`, refused unless a key there opens `what`.
async fn readable(host: &VaultHost, what: &str, runner: &Runner) -> Result<Value, String> {
    let doctor = host.skarbiec(&["keys", "check"], None, runner).await?;
    match doctor.get("status").and_then(Value::as_str) {
        Some(super::READABLE) => Ok(doctor),
        _ => Err(format!("{}'s keys check does not read {what}: {doctor}", host.target.name)),
    }
}
