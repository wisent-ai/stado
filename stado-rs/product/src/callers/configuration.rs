//! The incoming program's verdict on this host's configuration, before it
//! replaces the program that is running this installation.
//!
//! An incoming Stado reads this host's configuration with its own rules, and
//! a rule the configuration does not meet closes the host's boundaries only
//! once that binary runs: a release that refused the configuration left a
//! control host's object API answering `503 object authorization unavailable`,
//! and a release that required SSH keepalive for the resolver's tunnel left
//! every resolver adapter on its host answering `502`. So the incoming binary
//! judges this host first (`config validate`: the document, and what this
//! host's running roles need from it), and one it refuses is not placed.
//!
//! A release that renames a configuration key ships the migration with it
//! (`config migrate`). Validating with the incoming rules but not its
//! migrations would refuse every host that still held the old key, so no host
//! could ever receive the binary that migrates it; the incoming binary
//! therefore migrates and validates again before refusing, with both answers.

use std::path::Path;
use std::process::{Command, Output};

use anyhow::{bail, Context, Result};

/// Whether placing `destination` replaces the program running this
/// installation: the host's own Stado, the one whose configuration rules
/// decide whether this host keeps answering.
pub fn replaces_running_program(destination: &Path) -> bool {
    match std::env::current_exe() {
        Ok(running) => running.file_name() == destination.file_name(),
        Err(_) => false,
    }
}

/// Refuse `candidate` when it does not accept this host's configuration,
/// after its own migration. What it noted while accepting it (`NOTE ...`
/// lines: a resolver state left by a resolver that no longer runs) is said.
pub fn refuse_unaccepted(candidate: &Path, name: &str) -> Result<()> {
    let first = run(candidate, &["config", "validate"], name)?;
    if first.status.success() {
        say_notes(&first, name);
        return Ok(());
    }
    let migration = run(candidate, &["config", "migrate"], name)?;
    let second = run(candidate, &["config", "validate"], name)?;
    if second.status.success() {
        eprintln!(
            "the incoming {name} migrated this host's configuration before installing: {}",
            String::from_utf8_lossy(&migration.stdout).trim()
        );
        say_notes(&second, name);
        return Ok(());
    }
    bail!(
        "the incoming {name} refuses this host's configuration, so the installed {name} was \
         left in place: {}{}Its own migration (`{name} config migrate`) answered: {}{}Declare \
         or migrate what it names and install again.",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr),
        String::from_utf8_lossy(&migration.stdout),
        String::from_utf8_lossy(&migration.stderr),
    )
}

/// Each `NOTE ...` line of an accepting `config validate`, under the name of
/// the program that wrote it.
fn say_notes(validated: &Output, name: &str) {
    for note in String::from_utf8_lossy(&validated.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("NOTE "))
    {
        eprintln!("the incoming {name} accepts this host's configuration: {note}");
    }
}

fn run(candidate: &Path, args: &[&str], name: &str) -> Result<Output> {
    stado_wait::output(Command::new(candidate).args(args)).with_context(|| {
        format!(
            "cannot run the incoming {name} ({}) on this host's configuration",
            args.join(" ")
        )
    })
}
