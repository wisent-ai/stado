//! `stado release` — operator surface for [`crate::release`]: ask what the next
//! version is instead of deciding it from memory while publishing.
//!
//! The published artifact is the evidence. Because the release channel is
//! immutable and its downloads are bearer-free, the currently published binary
//! can always be fetched and asked what it can do. That is what makes the
//! comparison mechanical rather than a recollection of what changed.
//!
//! NO Python original: nothing there ever decided a version.

use clap::{Args, Subcommand};
use serde_json::{json, Value};

use super::table::print as print_table;
use super::CmdError;
use crate::release::{decide, Change, Surface, Version};

#[derive(Subcommand)]
pub enum ReleaseCommands {
    /// Decide the next version by comparing a candidate build's commands against
    /// the published build's, and say which rule produced the answer.
    Next(NextArgs),
    /// Print one build's observable command surface.
    Surface(SurfaceArgs),
}

#[derive(Args)]
pub struct NextArgs {
    /// The version currently published, as a major.minor.patch triple.
    #[arg(long)]
    current: String,
    /// Executable already published at --current.
    #[arg(long)]
    published: String,
    /// Executable being considered for release.
    #[arg(long)]
    candidate: String,
    /// Declare breakage the command list cannot show: a field dropped from a
    /// payload, a stored format changed, an exit code repurposed. This can only
    /// escalate the classification, never lower it.
    #[arg(long)]
    breaking: bool,
    /// Subcommand each executable answers with its command list.
    #[arg(long, default_value = "help")]
    surface_command: String,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct SurfaceArgs {
    /// Executable to interrogate.
    binary: String,
    #[arg(long, default_value = "help")]
    surface_command: String,
    #[arg(long)]
    json: bool,
}

pub async fn dispatch(command: ReleaseCommands) -> Result<(), CmdError> {
    match command {
        ReleaseCommands::Next(args) => next(&args),
        ReleaseCommands::Surface(args) => surface(&args),
    }
}

/// Ask a build what it can do. Run rather than read out of a source tree, because
/// the artifact is the thing being released and a checkout is not.
fn read_surface(binary: &str, surface_command: &str) -> Result<Surface, CmdError> {
    let output = std::process::Command::new(binary)
        .arg(surface_command)
        .output()
        .map_err(|err| CmdError::click(format!("{binary} {surface_command}: {err}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(CmdError::click(format!(
            "{binary} {surface_command} failed: {}",
            stderr.trim()
        )));
    }
    let body = String::from_utf8_lossy(&output.stdout);
    Surface::from_help(&body)
        .map_err(|err| CmdError::click(format!("{binary} {surface_command}: {err}")))
}

fn next(args: &NextArgs) -> Result<(), CmdError> {
    let current = Version::parse(&args.current).map_err(|err| CmdError::click(err.to_string()))?;
    let published = read_surface(&args.published, &args.surface_command)?;
    let candidate = read_surface(&args.candidate, &args.surface_command)?;
    let decision = decide(current, &published, &candidate, args.breaking);

    if args.json {
        return echo_json(&json!({
            "current": decision.current.to_string(),
            "next": decision.next.to_string(),
            "change": decision.change.as_str(),
            "added": decision.diff.added,
            "removed": decision.diff.removed,
            "declared_breaking": args.breaking,
            "unstable": decision.current.is_unstable(),
        }));
    }

    let mut rows = vec![
        vec!["current".to_string(), decision.current.to_string()],
        vec!["change".to_string(), decision.change.as_str().to_string()],
        vec!["next".to_string(), decision.next.to_string()],
    ];
    if !decision.diff.removed.is_empty() {
        rows.push(vec![
            "removed".to_string(),
            decision.diff.removed.join(", "),
        ]);
    }
    if !decision.diff.added.is_empty() {
        rows.push(vec!["added".to_string(), decision.diff.added.join(", ")]);
    }
    if args.breaking {
        rows.push(vec![
            "declared".to_string(),
            "breaking, by the operator".to_string(),
        ]);
    }
    print_table(&["FIELD", "VALUE"], &rows);

    // Name the rule that produced the number, so the answer can be argued with
    // instead of taken on faith.
    let reason = match (decision.change, decision.current.is_unstable()) {
        (Change::Breaking, true) => {
            "a removed or redefined contract, and under 0.x the minor slot carries compatibility"
        }
        (Change::Breaking, false) => "a removed or redefined contract",
        (Change::Additive, true) => {
            "added commands only, which under 0.x is a compatible change, so it lands in patch"
        }
        (Change::Additive, false) => "added commands only",
        (Change::Internal, _) => "an identical command surface",
    };
    println!("\n{reason}.");
    Ok(())
}

fn surface(args: &SurfaceArgs) -> Result<(), CmdError> {
    let surface = read_surface(&args.binary, &args.surface_command)?;
    if args.json {
        return echo_json(&json!({
            "binary": args.binary,
            "commands": surface.commands,
        }));
    }
    for command in &surface.commands {
        println!("{command}");
    }
    Ok(())
}

/// Same shape as `cli/storage.rs::echo_json`.
fn echo_json(value: &Value) -> Result<(), CmdError> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
