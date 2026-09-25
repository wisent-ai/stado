mod catalog;
mod lifecycle;
mod native;
pub(crate) mod registry;
use crate::common::Runtime;
use anyhow::{Context, Result};
use clap::{Arg, ArgAction, Command};
use std::path::PathBuf;

fn value(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name).long(name).help(help).num_args(1)
}
fn flag(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name)
        .long(name)
        .help(help)
        .action(ArgAction::SetTrue)
}
fn positional(name: &'static str) -> Arg {
    Arg::new("positional").value_name(name)
}

/// Add the product arguments and operations to Stado's `product` command.
pub fn augment(command: Command) -> Command {
    command
        .arg(value("catalog", "Independent authoritative catalog path").global(true))
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(catalog::command())
        .subcommand(registry::command())
        .subcommand(registry::creation())
        .subcommand(lifecycle::installation(
            "install",
            "Install one product surface from its canonical recipe or exact qualified release",
        ))
        .subcommand(lifecycle::installation(
            "update",
            "Update one product surface while retaining exact rollback bytes",
        ))
        .subcommand(lifecycle::installation(
            "status",
            "Inspect recorded installation, source, signatures and actual readiness",
        ))
        .subcommand(lifecycle::installation(
            "remove",
            "Remove only owned paths and the declared service surface",
        ))
        .subcommand(lifecycle::installation(
            "rollback",
            "Restore exact retained installation bytes without rebuilding",
        ))
        .subcommand(lifecycle::sync())
        .subcommand(lifecycle::schedule())
        .subcommand(lifecycle::signing())
        .subcommand(
            Command::new("paths")
                .about("Inspect actual executable ownership and PATH collisions")
                .arg(flag("json", "Print observed path collisions")),
        )
        .subcommand(native::cargo())
        .subcommand(native::swift())
        .subcommand(native::documentation())
}

/// Run one parsed `stado product` invocation as `build`, returning its exit status.
pub fn run(mut matches: clap::ArgMatches, build: crate::Build) -> Result<i32> {
    let _ = crate::BUILD.set(build);
    let runtime = Runtime::new(matches.get_one::<String>("catalog").map(PathBuf::from))?;
    let (action, mut arguments) = matches
        .remove_subcommand()
        .context("product command is missing")?;
    match action.as_str() {
        "catalog" => crate::catalog::run(arguments, &runtime),
        "create" => crate::creation::run(arguments, &runtime),
        "registry" => {
            let (action, arguments) = arguments
                .remove_subcommand()
                .context("registry operation is missing")?;
            crate::registry::run(&action, arguments, &runtime)
        }
        "install" | "update" | "status" | "remove" | "rollback" | "sync" => {
            crate::install::run(&action, arguments, &runtime)
        }
        "schedule" => crate::schedule::run(arguments, &runtime),
        "signing" => {
            let (action, arguments) = arguments
                .remove_subcommand()
                .context("signing operation is missing")?;
            crate::signing::run(&action, arguments, &runtime)
        }
        "paths" => crate::paths::run(arguments, &runtime),
        "cargo" => crate::cargo::run(arguments, &runtime),
        "swift" => crate::native::run(arguments, &runtime),
        "documentation" => {
            let (action, arguments) = arguments
                .remove_subcommand()
                .context("documentation operation is missing")?;
            crate::documentation::run(&action, arguments, &runtime)
        }
        _ => unreachable!("Clap admitted an undeclared product command"),
    }
}
