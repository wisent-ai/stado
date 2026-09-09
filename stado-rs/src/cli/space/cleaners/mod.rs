//! `stado space cleaners`: which janitor cleaners a host declares, and the
//! typed write that arms one.
//!
//! The mechanism that holds a host above its watermark is a declared cleaner,
//! and until this command the only way to arm one was to hand-edit the
//! canonical registry document. That is why `charless-mac-mini` sat 7.6 GiB
//! below its declared target on 2026-09-09 with 52.4 GiB of published release
//! versions in `~/.stado/local-storage` and 10.4 GiB of replica objects in
//! `~/.stado/local-backup`: this binary implements `release_store` and
//! `backup_twins`, which sweep exactly those roots, and that host declared
//! neither. Nothing was broken and nothing was missing except the declaration,
//! which no command could write.
//!
//! Two refusals are the whole safety of the write. A name this product does
//! not implement is refused with the list that may be declared, and a name the
//! target's installed binary predates is refused with the version it needs —
//! because a registry policy is one document read by every release at once,
//! and a name an older binary cannot parse made that host read `cleaners:
//! null` and switch off every cleaner it was already running.

use clap::{Args, Subcommand};
use serde_json::{json, Map, Value};

use super::{print_json, CmdError};
use crate::providers::local::disk_cleanup::catalogue;

mod write;

#[derive(Subcommand)]
pub enum CleanerCommands {
    /// Read every cleaner this product implements against what TARGET declares.
    List {
        target: String,
        #[arg(long)]
        json: bool,
    },
    /// Declare one cleaner for TARGET in the canonical registry.
    Declare(DeclareArgs),
    /// Withdraw one declared cleaner from TARGET.
    Remove {
        target: String,
        /// The cleaner name to withdraw.
        #[arg(long)]
        cleaner: String,
        #[arg(long)]
        json: bool,
    },
}

/// Everything `space cleaners declare` accepts.
#[derive(Args)]
pub struct DeclareArgs {
    pub target: String,
    /// The cleaner to arm; `stado space cleaners list TARGET` names them.
    #[arg(long)]
    pub cleaner: String,
    /// Where it sweeps, absolute or home-relative. Omitted leaves the
    /// cleaner's own default root.
    #[arg(long)]
    pub root: Option<String>,
    /// Nothing younger than this many seconds is a candidate.
    #[arg(long = "min-age-seconds")]
    pub min_age_seconds: Option<i64>,
    /// How many newest items survive with no other reason; `release_store`
    /// reads it as the rollback ladder it keeps per product.
    #[arg(long = "keep-newest")]
    pub keep_newest: Option<i64>,
    /// Permit taking an item whose upload to the object store is unproven.
    #[arg(long = "allow-missing-upload-proof", num_args = 1)]
    pub allow_missing_upload_proof: Option<bool>,
    #[arg(long)]
    pub json: bool,
}

/// The declared policy and the installed binary version for one target.
struct Declared {
    policy: Option<Value>,
    installed: String,
}

async fn declared_for(target: &str) -> Result<Declared, CmdError> {
    let registry = crate::targets::load_registry_auto()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let entry = registry
        .lookup(target)
        .ok_or_else(|| CmdError::click(format!("target not in registry: {target}")))?;
    Ok(Declared {
        policy: entry
            .disk_cleanup
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?,
        installed: entry
            .managed_versions
            .get("stado")
            .cloned()
            .unwrap_or_default(),
    })
}

/// One row per implemented cleaner: what it sweeps, whether this host declares
/// it, and — when it does not — whether the installed binary could take it.
fn rows(declared: &Declared) -> Vec<Value> {
    let cleaners = declared
        .policy
        .as_ref()
        .and_then(|policy| policy.get("cleaners"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    catalogue::CLEANERS
        .iter()
        .map(|entry| {
            let declaration = cleaners.get(entry.name);
            let supported = catalogue::version_at_least(&declared.installed, entry.since);
            json!({
                "cleaner": entry.name,
                "declared": declaration.is_some(),
                "declaration": declaration,
                "sweeps": entry.sweeps,
                "default_root": entry.default_root,
                "since": entry.since,
                "supported_by_installed_binary": supported,
                "detail": detail(entry, declaration.is_some(), supported, &declared.installed),
            })
        })
        .collect()
}

fn detail(
    entry: &catalogue::CleanerDeclaration,
    declared: bool,
    supported: bool,
    installed: &str,
) -> String {
    if declared {
        return format!("declared: this host sweeps {}", entry.sweeps);
    }
    if supported {
        return format!(
            "this product implements it and this host does not declare it; arm it with `stado space cleaners declare <target> --cleaner {}`",
            entry.name
        );
    }
    format!(
        "undeclared, and the binary installed here ({}) predates it: deliver at least {} first",
        if installed.is_empty() {
            "unknown"
        } else {
            installed
        },
        entry.since
    )
}

pub async fn dispatch(command: CleanerCommands) -> Result<(), CmdError> {
    match command {
        CleanerCommands::List { target, json } => list(&target, json).await,
        CleanerCommands::Declare(args) => declare(args).await,
        CleanerCommands::Remove {
            target,
            cleaner,
            json,
        } => write::write_cleaner(&target, &cleaner, None, json).await,
    }
}

async fn list(target: &str, json_output: bool) -> Result<(), CmdError> {
    let declared = declared_for(target).await?;
    let rows = rows(&declared);
    if json_output {
        return print_json(&json!({
            "target": target,
            "installed_stado": declared.installed,
            "declares_policy": declared.policy.is_some(),
            "cleaners": rows,
        }));
    }
    if declared.policy.is_none() {
        println!(
            "{target}: declares no disk_cleanup policy, so it is measured against the reporting default and arms nothing"
        );
    }
    for row in &rows {
        println!(
            "{}\t{}\t{}",
            row["cleaner"].as_str().unwrap_or_default(),
            if row["declared"] == json!(true) {
                "declared"
            } else {
                "undeclared"
            },
            row["detail"].as_str().unwrap_or_default(),
        );
    }
    Ok(())
}

async fn declare(args: DeclareArgs) -> Result<(), CmdError> {
    let entry = catalogue::cleaner(&args.cleaner).ok_or_else(|| {
        CmdError::usage(format!(
            "{} is not a cleaner this product implements; declare one of: {}",
            args.cleaner,
            catalogue::names().join(", ")
        ))
    })?;
    let declared = declared_for(&args.target).await?;
    if !catalogue::version_at_least(&declared.installed, entry.since) {
        return Err(CmdError::click(format!(
            "{} runs stado {} and {} first ships in {}: declaring it now makes that host reject its whole policy, so deliver the binary first with `stado release host-state --host {} --apply`",
            args.target,
            if declared.installed.is_empty() {
                "an unreadable version".to_string()
            } else {
                declared.installed.clone()
            },
            args.cleaner,
            entry.since,
            args.target
        )));
    }
    let mut fields = Map::new();
    if let Some(root) = args.root.as_ref() {
        fields.insert("root".to_string(), Value::from(root.clone()));
    }
    // `min_age_seconds` is required by the registry contract and floored per
    // cleaner, so a declaration that names none is written with that cleaner's
    // own floor rather than refused for a field an operator cannot guess.
    fields.insert(
        "min_age_seconds".to_string(),
        Value::from(args.min_age_seconds.unwrap_or(entry.min_age_floor_seconds)),
    );
    if let Some(keep) = args.keep_newest {
        fields.insert("keep_newest".to_string(), Value::from(keep));
    }
    if let Some(allow) = args.allow_missing_upload_proof {
        fields.insert("allow_missing_upload_proof".to_string(), Value::from(allow));
    }
    write::write_cleaner(
        &args.target,
        &args.cleaner,
        Some(Value::Object(fields)),
        args.json,
    )
    .await
}
