//! The `stado product changelog` command: its arguments and which move they
//! ask for.

use anyhow::{Context, Result};
use clap::{Arg, ArgAction, ArgGroup, ArgMatches, Command};
use std::path::Path;

pub fn command() -> Command {
    Command::new("changelog")
        .about("Move CHANGELOG.md's Unreleased entries into changelog/: as the given release in the version-bump commit, or as spilled parts while the release is prepared")
        .arg(Arg::new("version").long("version").help("The release the entries, spilled parts included, go out in"))
        .arg(
            Arg::new("spill")
                .long("spill")
                .action(ArgAction::SetTrue)
                .help("Move the entries written so far into changelog/unreleased/part-N.md, linked under Unreleased, so CHANGELOG.md takes new entries again"),
        )
        .group(ArgGroup::new("move").args(["version", "spill"]).required(true))
        .arg(
            Arg::new("line-limit")
                .long("line-limit")
                .required(true)
                .value_parser(clap::value_parser!(usize))
                .help("The repository's file length limit; no file the move writes passes it"),
        )
        .arg(Arg::new("root").long("root").default_value(".").help("The repository checkout"))
}

pub fn run(arguments: &ArgMatches) -> Result<i32> {
    let root = Path::new(
        arguments
            .get_one::<String>("root")
            .context("changelog root is missing")?,
    );
    let line_limit = *arguments
        .get_one::<usize>("line-limit")
        .context("--line-limit, the repository's file length limit, is missing")?;
    match arguments.get_one::<String>("version") {
        Some(version) => super::run(root, version, line_limit),
        None => super::spill::run(root, line_limit),
    }
}
