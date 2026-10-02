//! The incoming Stado's verdict on this host's configuration, after the
//! incoming Stado's own migrations.
//!
//! A release that renames a configuration key ships the migration with it
//! (`stado config migrate-identities`). Validating the host's file with the
//! incoming rules but not its migrations refused every host that still held
//! the old key — `agent.skarbiec.items` became `agent.skarbiec.roles`, and
//! every delivery of the release that made the change was refused on every
//! host, so no host could ever receive the binary that migrates it. The
//! incoming binary therefore migrates first; the migration validates before it
//! writes and keeps the previous file beside it, and a configuration it still
//! refuses is refused here with both answers.

use std::path::Path;
use std::process::Command;

use crate::cli::CmdError;

/// Validate this host's configuration with `staged`, migrating it with the
/// same binary when the first validation refuses it.
pub(super) fn admit(staged: &Path, name: &str) -> Result<(), CmdError> {
    let first = run(staged, &["config", "validate"], name)?;
    if first.status.success() {
        return Ok(());
    }
    let migration = run(staged, &["config", "migrate-identities"], name)?;
    let second = run(staged, &["config", "validate"], name)?;
    if second.status.success() {
        eprintln!(
            "the incoming {name} migrated this host's configuration before installing: {}",
            String::from_utf8_lossy(&migration.stdout).trim()
        );
        return Ok(());
    }
    Err(CmdError::click(format!(
        "the incoming {name} refuses this host's configuration, so the installed {name} was \
         left in place: {}{}Its own migration (`{name} config migrate-identities`) answered: \
         {}{}Migrate what it names and deliver again.",
        String::from_utf8_lossy(&second.stdout),
        String::from_utf8_lossy(&second.stderr),
        String::from_utf8_lossy(&migration.stdout),
        String::from_utf8_lossy(&migration.stderr),
    )))
}

fn run(staged: &Path, args: &[&str], name: &str) -> Result<std::process::Output, CmdError> {
    Command::new(staged).args(args).output().map_err(|error| {
        CmdError::click(format!(
            "cannot run the incoming {name} ({}) on this host's configuration: {error}",
            args.join(" ")
        ))
    })
}
