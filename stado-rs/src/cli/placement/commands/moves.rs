//! `stado placement move` — the operator half: resolve the profile, decide
//! whether this host may run the transaction at all, claim it through registry
//! CAS, then print the receipt or the refusal with its rollback detail.
//!
//! The transaction itself is [`relocate`], shared with the autonomy cycle's
//! placement relief, which moves a profile off a host over its memory
//! watermark with the same claim, the same execution and the same rollback.
//! A second executor there would be a second set of rollback rules for one
//! transaction.

use std::process::Stdio;

use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};

use crate::cli::placement::candidates::{
    declared_profile_hosts, ensure_profile_lifecycle_mutable, parse_registry, profile_host, target,
};
use crate::cli::placement::transfer::{
    cleanup_state_backup, execute_move, production_committer, release_claim, rollback, MoveContext,
    Progress,
};
use crate::cli::{registry, CmdError};
use crate::deploy::production_runner;
use crate::placement::{self, PlacementProfile, PlacementTransaction};
use crate::targets::Registry;

async fn delegate_to_registry_authority(
    document: &Value,
    registry: &Registry,
    requested: &[String],
    to_host: &str,
    json_output: bool,
) -> Result<bool, CmdError> {
    let Some(directory) =
        crate::service_resolution::directory(document).map_err(CmdError::click)?
    else {
        return Ok(false);
    };
    if local_is_authority(document, registry)
        .map_err(CmdError::click)?
        .unwrap_or(true)
    {
        return Ok(false);
    }
    let authority = registry
        .lookup(&directory.authority.target)
        .ok_or_else(|| CmdError::click("registry authority target disappeared"))?;
    let runner = crate::deploy::production_runner();
    let connection = crate::deploy::host_channel::select_ssh_connection(authority, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut argv = vec![
        directory.authority.command,
        "placement".to_string(),
        "move".to_string(),
        "--to-host".to_string(),
        to_host.to_string(),
    ];
    if json_output {
        argv.push("--json".to_string());
    }
    argv.extend(requested.iter().cloned());
    let remote_command = argv
        .iter()
        .map(|argument| crate::deploy::shlex_quote(argument))
        .collect::<Vec<_>>()
        .join(" ");
    let mut ssh_argv = crate::deploy::host_channel::ssh_options(connection.destination);
    ssh_argv.insert(1, "-T".to_string());
    ssh_argv.push(remote_command);
    let key = crate::deploy::host_access::ssh_key::materialize(authority.channel_key())
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let ssh_argv = crate::deploy::host_access::ssh_key::add_identity(ssh_argv, &key)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let (program, arguments) = ssh_argv
        .split_first()
        .ok_or_else(|| CmdError::click("registry authority SSH channel is empty"))?;
    let status = tokio::process::Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await
        .map_err(|error| CmdError::click(format!("registry authority SSH failed: {error}")))?;
    if !status.success() {
        return Err(CmdError::click(format!(
            "registry authority placement exited with {status}"
        )));
    }
    Ok(true)
}

/// Which host a profile is placed on. `Err` names the refusal a move would
/// meet before touching any host, so a planner can read it without claiming.
pub(crate) fn placed_host(
    registry: &Registry,
    profile: &PlacementProfile,
) -> Result<String, String> {
    let sources = declared_profile_hosts(registry, profile).map_err(|error| error.to_string())?;
    match sources.as_slice() {
        [source] => Ok(source.clone()),
        [] => Err(format!(
            "placement profile {:?} has no complete managed source",
            profile.name
        )),
        _ => Err(format!(
            "placement profile {:?} is active on multiple hosts: {}",
            profile.name,
            sources.join(", ")
        )),
    }
}

/// Whether the host this binary runs on is the directory authority, the one
/// host allowed to commit a placement transaction. `Ok(None)` is a document
/// with no directory: every host may then run the transaction itself.
pub(crate) fn local_is_authority(
    document: &Value,
    registry: &Registry,
) -> Result<Option<bool>, String> {
    let Some(directory) = crate::service_resolution::directory(document)? else {
        return Ok(None);
    };
    let hostname = crate::providers::vast::system_hostname();
    let local = registry
        .lookup_self(&hostname)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("placement host {hostname:?} has no registry target identity"))?;
    Ok(Some(local.name == directory.authority.target))
}

/// What one relocation left behind.
pub(crate) struct MoveReceipt {
    pub(crate) status: &'static str,
    pub(crate) transaction_id: Option<String>,
    pub(crate) profile: String,
    pub(crate) from_host: String,
    pub(crate) to_host: String,
    pub(crate) registry_generation: Option<String>,
}

/// Move `profile` to `to_host` from where it is placed now, on this host.
///
/// The caller has already decided this host may run the transaction (it is
/// the authority, or the document has none). `document` and `generation` are
/// the registry read the decision was made on; the claim is compare-and-swapped
/// against that generation, so a document that changed underneath is a
/// refusal, never a move over someone else's commit.
pub(crate) async fn relocate(
    document: Value,
    generation: String,
    profile: PlacementProfile,
    to_host: &str,
) -> Result<MoveReceipt, String> {
    let parsed_registry = parse_registry(&document).map_err(|error| error.to_string())?;
    profile_host(&profile, to_host).map_err(|error| error.to_string())?;
    let destination = target(&parsed_registry, to_host)
        .map_err(|error| error.to_string())?
        .clone();
    let source_name = placed_host(&parsed_registry, &profile)?;
    if source_name == to_host {
        return Ok(MoveReceipt {
            status: "already_placed",
            transaction_id: None,
            profile: profile.name,
            from_host: source_name,
            to_host: to_host.to_string(),
            registry_generation: None,
        });
    }
    let source = target(&parsed_registry, &source_name)
        .map_err(|error| error.to_string())?
        .clone();
    let transaction = PlacementTransaction {
        id: uuid::Uuid::new_v4().to_string(),
        profile: profile.name.clone(),
        from_host: source.name.clone(),
        to_host: destination.name.clone(),
        started_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
    };
    let mut claimed_document = document;
    placement::claim_transaction(&mut claimed_document, &transaction)?;
    let claim_generation = registry::push_document_if(&claimed_document, &generation)
        .await
        .map_err(|error| error.to_string())?;
    let context = MoveContext {
        profile,
        source,
        destination,
        registry: parsed_registry,
        claimed_document,
        claim_generation,
        transaction,
    };
    let runner = production_runner();
    let committer = production_committer();
    let mut progress = Progress::default();
    match execute_move(&context, &mut progress, &runner, &committer).await {
        Ok(committed_generation) => {
            for path in &progress.destination_written {
                if let Err(error) = cleanup_state_backup(
                    &context.destination,
                    path,
                    &context.transaction.id,
                    &runner,
                )
                .await
                {
                    eprintln!("Warning: {error}");
                }
            }
            Ok(MoveReceipt {
                status: "moved",
                transaction_id: Some(context.transaction.id),
                profile: context.profile.name,
                from_host: context.source.name,
                to_host: context.destination.name,
                registry_generation: Some(committed_generation),
            })
        }
        Err(primary) => {
            let rollback_errors = rollback(&context, &progress, &runner).await;
            let release_error = release_claim(&context.transaction.id).await.err();
            let mut details = vec![primary.to_string()];
            if !rollback_errors.is_empty() {
                details.push(format!("rollback failures: {}", rollback_errors.join("; ")));
            }
            if let Some(error) = release_error {
                details.push(format!("registry lock release failed: {error}"));
            }
            Err(details.join("; "))
        }
    }
}

pub(super) async fn move_services(
    requested: &[String],
    to_host: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let (document, generation) = registry::fetch_versioned_document().await?;
    crate::targets::validate_registry(&document)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let parsed_registry = parse_registry(&document)?;
    let profile = placement::profile_for_services(&document, requested).map_err(CmdError::click)?;
    ensure_profile_lifecycle_mutable(&profile)?;
    if delegate_to_registry_authority(&document, &parsed_registry, requested, to_host, json_output)
        .await?
    {
        return Ok(());
    }
    let receipt = relocate(document, generation, profile, to_host)
        .await
        .map_err(CmdError::click)?;
    let report = json!({
        "status": receipt.status,
        "transaction_id": receipt.transaction_id,
        "profile": receipt.profile,
        "from_host": receipt.from_host,
        "to_host": receipt.to_host,
        "registry_generation": receipt.registry_generation,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if receipt.status == "already_placed" {
        println!(
            "{} is already placed on {}",
            receipt.profile, receipt.to_host
        );
    } else {
        println!(
            "moved {} from {} to {} (registry generation {})",
            receipt.profile,
            receipt.from_host,
            receipt.to_host,
            receipt.registry_generation.as_deref().unwrap_or_default()
        );
    }
    Ok(())
}
