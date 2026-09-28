//! `stado release version-gate …`: the steps of the pull-request version gate
//! in `.github/workflows/version-check.yml`, run by the candidate binary
//! itself. They were embedded Python; each is now one subcommand here.
//!
//! - `surface` reads the top-level commands a binary advertises;
//! - `baseline` derives the published surface from verified release bytes;
//! - `decide` is the fleet's versioning rule (AutoVersion SPEC v0.1.0), and
//!   `conformance` proves it against AutoVersion's shared fixtures;
//! - `semver-at-least` compares a declared version with the required one;
//! - `unreachable-modules` finds `.rs` files no `mod` declaration reaches.

mod baseline;
mod conformance;
mod modules;
mod rule;
mod surface;

use std::path::PathBuf;

use clap::Subcommand;

use crate::cli::{CmdError, CLICK_ERROR_CODE};
use crate::primitives::failure::FailureCode;

#[derive(Subcommand)]
pub enum VersionGateCommands {
    /// Print `{"surface": [...]}`: the top-level commands BINARY advertises
    /// in `help`, or those in a saved help text.
    Surface {
        #[arg(
            long,
            conflicts_with = "help_text",
            required_unless_present = "help_text"
        )]
        binary: Option<PathBuf>,
        #[arg(long)]
        help_text: Option<PathBuf>,
    },
    /// Print `stado:VERSION` for the newest whole release the channel serves,
    /// or `bootstrap:VERSION` on an empty one; with --output, also write the
    /// baseline document. Run from the repository root.
    Baseline {
        /// The Stado binary that reads the channel.
        #[arg(long)]
        stado: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Classify the change between two surfaces and name the next version.
    Decide {
        #[arg(long)]
        current: String,
        #[arg(long)]
        published_surface: PathBuf,
        #[arg(long)]
        candidate_surface: PathBuf,
        /// Declare a break the surface cannot show; it may only escalate.
        #[arg(long)]
        breaking: bool,
        #[arg(long)]
        json: bool,
    },
    /// Reproduce every case of AutoVersion's FIXTURES.md with this port;
    /// fails when any class, next version or refusal differs.
    Conformance {
        #[arg(long)]
        fixtures: PathBuf,
    },
    /// Succeed when ACTUAL is a valid SemVer at least MINIMUM; a lower one
    /// fails, and an invalid one is refused with the usage status.
    SemverAtLeast { actual: String, minimum: String },
    /// Fail when a `.rs` file under the crate's `src/` is reachable from no
    /// crate root, or when a --known entry names a file that is gone.
    UnreachableModules {
        #[arg(long = "crate", default_value = ".")]
        crate_dir: PathBuf,
        #[arg(long)]
        known: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
}

fn surface_file(path: &std::path::Path) -> Result<Vec<String>, CmdError> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| CmdError::click(format!("{}: {error}", path.display())))?;
    let document: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| CmdError::click(format!("{}: {error}", path.display())))?;
    document["surface"]
        .as_array()
        .and_then(|names| {
            names
                .iter()
                .map(|name| name.as_str().map(str::to_string))
                .collect()
        })
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: `surface` is not a list of names",
                path.display()
            ))
        })
}

/// A negative answer the workflow branches on: exit 1 with nothing more to
/// print, since the command already said why on stdout.
fn negative() -> CmdError {
    CmdError::silent(CLICK_ERROR_CODE)
}

pub fn dispatch(command: VersionGateCommands) -> Result<(), CmdError> {
    match command {
        VersionGateCommands::Surface { binary, help_text } => {
            let commands = match (binary, help_text) {
                (Some(binary), _) => surface::of_binary(&binary),
                (None, Some(path)) => std::fs::read_to_string(&path)
                    .map_err(|error| format!("{}: {error}", path.display()))
                    .and_then(|text| surface::advertised(&text)),
                (None, None) => unreachable!("clap requires one source"),
            }
            .map_err(CmdError::click)?;
            println!("{}", surface::document(&commands));
            Ok(())
        }
        VersionGateCommands::Baseline { stado, output } => {
            match baseline::best(&stado, output.as_deref()) {
                Ok(answer) => {
                    println!("{answer}");
                    Ok(())
                }
                Err(baseline::Refusal::Invalid(detail)) => Err(CmdError::click(detail)),
                Err(baseline::Refusal::Unavailable(detail)) => Err(CmdError {
                    // The channel's own retryable answer; the entry point exits
                    // with the fleet's retry status, never as a verdict.
                    failure: Some(FailureCode::InfraDown),
                    ..CmdError::click(format!("the release channel cannot answer now: {detail}"))
                }),
            }
        }
        VersionGateCommands::Decide {
            current,
            published_surface,
            candidate_surface,
            breaking,
            json,
        } => {
            let answer = rule::decide(
                &current,
                &surface_file(&published_surface)?,
                &surface_file(&candidate_surface)?,
                breaking,
            )
            .map_err(|refusal| CmdError::click(refusal.to_string()))?;
            let document = serde_json::json!({
                "current": answer.current,
                "change": answer.change.name(),
                "next": answer.next,
                "removed": answer.removed,
                "added": answer.added,
            });
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&document).expect("decision serialises")
                );
            } else {
                println!(
                    "{} {} -> {}",
                    answer.change.name(),
                    answer.current,
                    answer.next
                );
            }
            Ok(())
        }
        VersionGateCommands::Conformance { fixtures } => {
            let text = std::fs::read_to_string(&fixtures)
                .map_err(|error| CmdError::usage(format!("{}: {error}", fixtures.display())))?;
            match conformance::run(&text) {
                Ok(true) => Ok(()),
                Ok(false) => Err(negative()),
                Err(detail) => Err(CmdError::usage(detail)),
            }
        }
        VersionGateCommands::SemverAtLeast { actual, minimum } => {
            match rule::semver_at_least(&actual, &minimum) {
                Ok(true) => Ok(()),
                Ok(false) => Err(negative()),
                Err(detail) => Err(CmdError::usage(detail)),
            }
        }
        VersionGateCommands::UnreachableModules {
            crate_dir,
            known,
            json,
        } => match modules::check(&crate_dir, known.as_deref(), json) {
            modules::Outcome::Clean => Ok(()),
            modules::Outcome::Findings => Err(negative()),
            modules::Outcome::Unreadable(detail) => Err(CmdError::usage(detail)),
        },
    }
}
