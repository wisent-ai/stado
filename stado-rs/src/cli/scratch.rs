//! `stado scratch`: lease a disposable target on a registered host.
//!
//! Profiles, lifetimes and mechanisms live in
//! `stado-rs/data/scratch-profiles.json`; this module is the command surface
//! over [`crate::deploy::scratch`] and the only place its reports are rendered
//! for a person. Adding a kind of disposable target is a declaration change,
//! not another CLI verb.

use std::path::PathBuf;

use clap::Subcommand;
use serde_json::{Map, Value};

use super::CmdError;
use crate::deploy::scratch::{self, declaration, LeaseRequest};

#[derive(Subcommand)]
pub enum ScratchCommands {
    /// Read every declared profile: mechanism, platforms and lifetimes.
    Profiles {
        /// Emit the declaration as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Lease a disposable target on HOST, and prove it can be entered.
    Create {
        /// Registry target the lease is taken on.
        #[arg(long)]
        host: String,
        /// Profile name from stado-rs/data/scratch-profiles.json.
        #[arg(long)]
        profile: String,
        /// Lease name, which is also the account name. Generated when omitted.
        #[arg(long)]
        name: Option<String>,
        /// Lifetime, in minutes or hours (90m, 2h). The profile's default when
        /// omitted, refused above its maximum.
        #[arg(long)]
        ttl: Option<String>,
        /// Directory the emitted registry is written to. Refused when it
        /// already exists.
        #[arg(long)]
        root: Option<PathBuf>,
        /// Emit the lease report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Read every lease HOST holds, with each account's real presence.
    List {
        /// Registry target to read.
        #[arg(long)]
        host: String,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Destroy one lease, and prove the host is clear of it.
    Destroy {
        /// Lease name.
        name: String,
        /// Registry target holding the lease.
        #[arg(long)]
        host: String,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Destroy every expired lease on HOST, previewing without --apply.
    Reap {
        /// Registry target to sweep.
        #[arg(long)]
        host: String,
        /// Destroy the expired leases instead of only reporting them.
        #[arg(long)]
        apply: bool,
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
}

pub async fn dispatch(command: ScratchCommands) -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    match command {
        ScratchCommands::Profiles { json } => {
            let declared = declaration::declaration().map_err(|exc| CmdError::click(exc.0))?;
            if json {
                print_json(&serde_json::json!({
                    "declaration": declaration::DECLARATION_PATH,
                    "schema": declared.schema,
                    "profiles": declared.profiles,
                }))?;
                return Ok(());
            }
            for profile in &declared.profiles {
                println!(
                    "{}\t{}\t{}\tshell {}\tttl {} (max {})",
                    profile.name,
                    profile.mechanism.as_str(),
                    profile.platforms.join(", "),
                    profile.shell,
                    profile.default_ttl,
                    profile.max_ttl
                );
                println!("\t{}", profile.summary);
            }
            Ok(())
        }
        ScratchCommands::Create {
            host,
            profile,
            name,
            ttl,
            root,
            json,
        } => {
            let request = LeaseRequest {
                target: host,
                profile,
                name,
                ttl,
                root,
            };
            let report = scratch::create(&request, &runner)
                .await
                .map_err(|exc| CmdError::click(exc.0))?;
            if json {
                return print_json(&Value::Object(report));
            }
            println!(
                "leased {} on {}",
                text(&report, "name"),
                text(&report, "target")
            );
            println!(
                "  profile      {} ({})",
                text(&report, "profile"),
                text(&report, "mechanism")
            );
            println!("  ssh          {}", text(&report, "ssh"));
            println!("  home         {}", text(&report, "home_path"));
            println!(
                "  expires      {} ({})",
                text(&report, "expires_at"),
                text(&report, "ttl")
            );
            println!("  registry     {}", text(&report, "registry_path"));
            println!(
                "  use it with  WC_STORAGE_BACKEND=local WC_LOCAL_STORAGE_PATH={}",
                text(&report, "storage_root")
            );
            let reaped = names(&report, "reaped");
            if !reaped.is_empty() {
                println!("  reaped       {}", reaped.join(", "));
            }
            Ok(())
        }
        ScratchCommands::List { host, json } => {
            let report = scratch::list(&host, &runner)
                .await
                .map_err(|exc| CmdError::click(exc.0))?;
            if json {
                return print_json(&Value::Object(report));
            }
            let leases = rows(&report);
            if leases.is_empty() {
                println!("{} holds no scratch leases", text(&report, "target"));
                return Ok(());
            }
            for lease in leases {
                println!(
                    "{}\t{}\taccount {}\t{}\t{}",
                    field(lease, "name"),
                    field(lease, "profile"),
                    field(lease, "account"),
                    remaining(lease),
                    field(lease, "requested_by")
                );
                if let Some(reason) = lease.get("unreadable").and_then(Value::as_str) {
                    println!("\tunreadable record: {reason}");
                }
            }
            Ok(())
        }
        ScratchCommands::Destroy { name, host, json } => {
            let report = scratch::destroy(&host, &name, &runner)
                .await
                .map_err(|exc| CmdError::click(exc.0))?;
            if json {
                return print_json(&Value::Object(report));
            }
            println!(
                "destroyed {} on {} at {}: account {}, home {}, record {}",
                text(&report, "name"),
                text(&report, "target"),
                text(&report, "destroyed_at"),
                text(&report, "account"),
                text(&report, "home"),
                text(&report, "record")
            );
            println!("  storage root {}", text(&report, "storage_root"));
            Ok(())
        }
        ScratchCommands::Reap { host, apply, json } => {
            let report = scratch::reap(&host, apply, &runner)
                .await
                .map_err(|exc| CmdError::click(exc.0))?;
            if json {
                return print_json(&Value::Object(report));
            }
            for lease in rows(&report) {
                println!(
                    "{}\t{}\texpires {}\t{}",
                    field(lease, "action"),
                    field(lease, "name"),
                    field(lease, "expires_at"),
                    field(lease, "account")
                );
            }
            println!(
                "{} destroyed, {} kept on {}{}",
                count(&report, "destroyed"),
                count(&report, "kept"),
                text(&report, "target"),
                if apply { "" } else { " (preview)" }
            );
            for failure in report
                .get("failures")
                .and_then(Value::as_array)
                .unwrap_or(&Vec::new())
            {
                println!(
                    "failed\t{}\t{}",
                    failure
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    failure
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                );
            }
            Ok(())
        }
    }
}

fn print_json(value: &Value) -> Result<(), CmdError> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|exc| CmdError::click(format!("report is not serializable: {exc}")))?;
    println!("{text}");
    Ok(())
}

fn text(report: &Map<String, Value>, key: &str) -> String {
    report
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn count(report: &Map<String, Value>, key: &str) -> String {
    report
        .get(key)
        .and_then(Value::as_u64)
        .map_or_else(|| "0".to_string(), |value| value.to_string())
}

fn names(report: &Map<String, Value>, key: &str) -> Vec<String> {
    report
        .get(key)
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn rows(report: &Map<String, Value>) -> Vec<&Map<String, Value>> {
    report
        .get("leases")
        .and_then(Value::as_array)
        .map(|entries| entries.iter().filter_map(Value::as_object).collect())
        .unwrap_or_default()
}

fn field(row: &Map<String, Value>, key: &str) -> String {
    match row.get(key) {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Bool(value)) => value.to_string(),
        Some(Value::Number(value)) => value.to_string(),
        _ => String::new(),
    }
}

/// How long one lease has left, in the operator's words rather than seconds:
/// a negative number is the thing an operator has to translate, and translating
/// it wrong is how an expired lease looks alive.
fn remaining(row: &Map<String, Value>) -> String {
    let expired = row
        .get("expired")
        .and_then(Value::as_bool)
        .unwrap_or_default();
    let seconds = row.get("seconds_remaining").and_then(Value::as_i64);
    match (expired, seconds) {
        (true, Some(value)) => format!("expired {} ago", age(-value)),
        (true, None) => "expired (undatable record)".to_string(),
        (false, Some(value)) => format!("{} left", age(value)),
        (false, None) => "lifetime unknown".to_string(),
    }
}

/// One spelling of a span, the registry's own.
fn age(seconds: i64) -> String {
    chrono::TimeDelta::try_seconds(seconds).map_or_else(
        || "an unreadable span".to_string(),
        super::registry::human_age,
    )
}
