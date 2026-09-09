//! Read and update cleaner declarations. Installed support is observed on the
//! target; `managed_versions` is desired state, not installed evidence.

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
    installed_error: Option<String>,
}

async fn declared_for(target: &str) -> Result<Declared, CmdError> {
    let registry = crate::targets::load_registry_auto()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let entry = registry
        .lookup(target)
        .ok_or_else(|| CmdError::click(format!("target not in registry: {target}")))?;
    let runner = crate::deploy::production_runner();
    let observed = crate::deploy::host_inventory::inventory_target(
        entry,
        registry.service_directory.as_ref(),
        &runner,
    )
    .await;
    let (installed, installed_error) = match observed {
        Ok(report) => {
            let version = report["managed_binaries"]
                .as_array()
                .and_then(|rows| rows.iter().find(|row| row["name"] == "stado"))
                .and_then(|row| row["version"].as_str())
                .and_then(|version| {
                    crate::deploy::host_inventory::reported_version("stado", version)
                })
                .map(str::to_string);
            let error = version.is_none().then(|| {
                report["error"]
                    .as_str()
                    .unwrap_or("the host inventory reported no readable installed Stado version")
                    .to_string()
            });
            (version.unwrap_or_default(), error)
        }
        Err(error) => (String::new(), Some(error.to_string())),
    };
    Ok(Declared {
        policy: entry
            .disk_cleanup
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?,
        installed,
        installed_error,
    })
}

/// One row per implemented cleaner: what it sweeps, whether this host declares
/// it, and — when it does not — whether the installed binary could take it.
fn rows(declared: &Declared) -> Vec<Value> {
    let cleaners = declared
        .policy
        .as_ref()
        .and_then(|policy| policy.get("cleaners"))
        .and_then(Value::as_object);
    catalogue::CLEANERS
        .iter()
        .map(|entry| {
            let declaration = cleaners.and_then(|rows| rows.get(entry.name));
            let supported = catalogue::version_at_least(&declared.installed, entry.since);
            json!({
                "cleaner": entry.name,
                "declared": declaration.is_some(),
                "declaration": declaration,
                "sweeps": entry.sweeps,
                "default_root": entry.default_root,
                "since": entry.since,
                "min_age_floor_seconds": entry.min_age_floor_seconds,
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
        return format!(
            "declared scan scope: {}; policy mode determines whether it can delete",
            entry.sweeps
        );
    }
    if installed.is_empty() {
        return "installed Stado version could not be observed; support is unknown".to_string();
    }
    if supported {
        return format!(
            "not declared; inspect or add its policy with `stado space cleaners declare <target> --cleaner {}`",
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
            "installed_read_error": declared.installed_error,
            "policy_mode": declared.policy.as_ref().and_then(|policy| policy.get("mode")),
            "declares_policy": declared.policy.is_some(),
            "cleaners": rows,
        }));
    }
    if let Some(error) = &declared.installed_error {
        println!("{target}: installed version unavailable: {error}");
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
    if let Some(error) = declared.installed_error {
        return Err(CmdError::click(format!(
            "cannot verify cleaner support on {}: {error}",
            args.target
        )));
    }
    if !catalogue::version_at_least(&declared.installed, entry.since) {
        return Err(CmdError::click(format!(
            "{} reports installed stado {}; {} requires at least {}; no policy was changed",
            args.target,
            if declared.installed.is_empty() {
                "an unreadable version".to_string()
            } else {
                declared.installed.clone()
            },
            args.cleaner,
            entry.since,
        )));
    }
    let mut fields = Map::new();
    if let Some(root) = args.root.as_ref() {
        fields.insert("root".to_string(), Value::from(root.clone()));
    }
    if let Some(seconds) = args.min_age_seconds {
        fields.insert("min_age_seconds".to_string(), Value::from(seconds));
    }
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
