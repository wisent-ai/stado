//! Read-only: `launchctl list`, `launchctl print` for the three domains,
//! `launchctl dumpstate` once, and the unit files those name. It starts
//! nothing, stops nothing, signals nothing and needs no sudo.
//!
//! Three reads, then one join, then one pass per label. The shape matters
//! because this script is what the fleet sweep spends its budget on. The
//! version before this one asked launchd again for every label -- five `awk`
//! passes over two in-memory tables and up to three `launchctl print
//! <domain>/<label>` calls each -- so a mac carrying a thousand labels would
//! spawn something near fifteen thousand processes to answer questions that
//! one pass answers for every label at once. That takes longer than
//! [`crate::deploy::host_recovery::TIMEOUT_SECONDS`], the channel kills it,
//! and `stado doctor` reports the host as `not measured` -- the host running
//! the sweep is the one host the sweep can never finish. One pass finishes
//! well inside the cap.

mod launchd;
mod posture;
mod systemd;

pub(crate) use launchd::LOADED_UNITS_SCRIPT;
pub(crate) use posture::PATH_POSTURE_SCRIPT;
use systemd::SYSTEMD_UNITS_SCRIPT;

/// How much one read of the host has to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LoadedDetails {
    /// Everything: environment keys, script reads, process ages, and the
    /// `stado` copies a shell on the host could find.
    Full,
    /// Only what image reconciliation needs: labels, domains, declared
    /// executables and running commands.
    Images,
}

/// The script for one read at the requested level of detail: the systemd
/// read on a Linux host, the launchd read everywhere else.
pub(crate) fn loaded_labels_script(details: LoadedDetails) -> String {
    let (word, posture) = match details {
        LoadedDetails::Full => ("full", PATH_POSTURE_SCRIPT),
        LoadedDetails::Images => ("images", ""),
    };
    format!("details={word}\n{SYSTEMD_UNITS_SCRIPT}{LOADED_UNITS_SCRIPT}{posture}")
}
