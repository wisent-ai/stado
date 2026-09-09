//! `stado host cron TARGET` — read, prune and restore one host's crontab.
//!
//! NO Python original.
//!
//! ## Why this exists
//!
//! On 2026-08-31 `charless-mac-mini` was cleaned of duplicate janitors and
//! duplicate queue agents: a launchd label retired with a verified
//! postcondition, its plist deleted, a stale user-domain job booted out of
//! `gui/501`. All of it correct, and all of it one reboot from coming back,
//! because the machine also carried four `@reboot` crontab entries that no
//! launchd domain and no registry document mentions:
//!
//! ```text
//! @reboot /bin/sh $HOME/.stado/bin/run-com.wisent.compute.coordinator.charless-control-plane.sh
//! @reboot /bin/sh $HOME/.stado/bin/start-stado-tailnet-object-proxy
//! @reboot /bin/sh $HOME/.stado/bin/run-com.wisent.compute.disk-cleanup.disk-cleanup.sh
//! @reboot /bin/sh $HOME/.stado/bin/run-com.wisent.compute.agent.charless-mac-mini.sh
//! ```
//!
//! Two of those resurrect the exact defects that session removed. A
//! retirement that survives `launchctl` and not a reboot is not a
//! retirement, and until this module existed the fleet could read that table
//! ([`crate::deploy::host_exec`]'s `crontab -l`) and had no sanctioned way to
//! change it — the only remaining answer was a bare `crontab -e` over ssh,
//! which nothing bounds and nobody audits.
//!
//! ## What it refuses
//!
//! The guards live on the host, for the reason
//! `cli::host::remove_file_document` gives: the table is what the host says
//! it is, not what the operator believes.
//!
//! - The substring must match EXACTLY ONE line. Zero is `absent`; two or more
//!   is refused with both lines printed, because a pattern that reaches more
//!   of a periodic table than its author meant is how an operator deletes a
//!   machine's boot sequence.
//! - That line must reference a path under `$HOME/.stado`. Everything else in
//!   a crontab belongs to somebody else — the entry that keeps a tailnet
//!   proxy alive is one line away from the entry that starts a duplicate
//!   agent, and only the fleet's own install root marks which is which.
//! - `--apply` writes the WHOLE current table to `$HOME/.stado/cron-backups`
//!   before installing the filtered one, and reports that path. Restoring is
//!   this same command with `--restore`, so both directions are product verbs
//!   and neither is a shell line an operator has to remember.
//! - Preview is the default. The table and the matched line come back
//!   base64-encoded either way, so a caller can keep a verbatim copy of what
//!   it is about to change.
//!
//! The components: `scripts` holds the two remote programs verbatim,
//! `outcome` is what one host answered, `parse` folds the host's marker
//! lines into that answer, and `verbs` runs a prune or a restore.

mod outcome;
mod parse;
mod scripts;
mod verbs;

pub use outcome::CronOutcome;
pub use verbs::{prune, restore};

/// The table was read and nothing was changed.
pub const STATE_READ: &str = "read";
/// The matched line is gone and the table is installed.
pub const STATE_PRUNED: &str = "pruned";
/// Nothing in the table matched.
pub const STATE_ABSENT: &str = "absent";
/// A guard refused; `detail` carries which one.
pub const STATE_REFUSED: &str = "refused";
/// A backup was restored over the live table.
pub const STATE_RESTORED: &str = "restored";

/// Where `--apply` keeps the table it replaced. Under the fleet's own install
/// root so `space file remove` can reach it and an operator
/// is never asked to trust `/tmp` with the boot sequence of a production box.
pub const BACKUP_DIR: &str = "$HOME/.stado/cron-backups";
