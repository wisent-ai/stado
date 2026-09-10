//! `stado scratch`: lease a disposable target on a registered host.
//!
//! Profiles, lifetimes and mechanisms live in
//! `stado-rs/data/work/scratch-profiles.json`; this module is the command surface
//! over [`crate::deploy::scratch`]. Adding a kind of disposable target is a
//! declaration change, not another CLI verb.

use std::path::PathBuf;

use clap::Subcommand;
use serde_json::Value;

use super::CmdError;
use crate::deploy::scratch::{self, declaration, LeaseRequest};

mod render;

use render::{count, field, flag, names, objects, print_json, remaining, text};

#[derive(Subcommand)]
pub enum ScratchCommands {
    /// Read every declared profile: mechanism, platforms and lifetimes.
    Profiles {
        /// Emit the declaration as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Read which registry hosts a lease may be taken on, and why the others
    /// may not.
    Hosts {
        /// Emit the report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Lease a disposable target on HOST, and prove it can be entered.
    Create {
        /// Registry target the lease is taken on.
        #[arg(long)]
        host: String,
        /// Profile name from stado-rs/data/work/scratch-profiles.json.
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
        ScratchCommands::Profiles { json } => profiles(json),
        ScratchCommands::Hosts { json } => hosts(json).await,
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
            // Before the run line, because it decides what can be delivered
            // to this lease: a `none:` answer means only a legacy-manifest
            // version will install here.
            println!("  trust        {}", text(&report, "release_trust"));
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
            let leases = objects(&report, "leases");
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
            for lease in objects(&report, "leases") {
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
            for failure in objects(&report, "failures") {
                println!(
                    "failed\t{}\t{}",
                    field(failure, "name"),
                    field(failure, "error")
                );
            }
            Ok(())
        }
    }
}

/// The declaration, as declared.
fn profiles(json: bool) -> Result<(), CmdError> {
    let declared = declaration::declaration().map_err(|exc| CmdError::click(exc.0))?;
    if json {
        return print_json(&serde_json::json!({
            "declaration": declaration::DECLARATION_PATH,
            "schema": declared.schema,
            "profiles": declared.profiles,
        }));
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

/// Where a lease can be taken, and why not everywhere.
async fn hosts(json: bool) -> Result<(), CmdError> {
    let report = scratch::hosts()
        .await
        .map_err(|exc| CmdError::click(exc.0))?;
    if json {
        return print_json(&Value::Object(report));
    }
    for host in objects(&report, "hosts") {
        let eligible = flag(host, "eligible");
        println!(
            "{}\t{}\t{}\t{}",
            if eligible { "leasable" } else { "refused" },
            field(host, "target"),
            field(host, "release_platform"),
            if eligible {
                format!("profile {}", field(host, "profile"))
            } else {
                field(host, "refusal")
            }
        );
    }
    Ok(())
}
