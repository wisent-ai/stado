//! Put the fleet back on the bytes the swap just installed: pick the
//! platform's service manager, and say which units recycle themselves.

use std::path::Path;

use super::launchd::recycle_launchd;
use super::systemd::recycle_systemd;

/// Reconcile OTHER managed units configured to run, or still executing,
/// the binaries this update just replaced.
///
/// Why this exists: [`replace_verified`] renames a new binary over the old
/// one, and only the process that ran the update re-execs itself. A unit that
/// is not that process keeps executing the inode it started with, for as long
/// as it lives, because neither launchd nor systemd has any reason to notice
/// that the file underneath it changed.
///
/// That is not theoretical. On 2026-09-01 the disk-cleanup janitor on
/// `lukasz-macbook` was executing a 68,977,488-byte image of
/// `~/.stado/bin/stado` while the file at that exact path was 70,892,848
/// bytes: the process had been up since 2026-08-27 and the binary was
/// replaced under it. Its reports named four cleaners and carried no
/// `writer_version`, while the installed build declares six and sets that
/// field, so the registry policy it was handed no longer validated. It
/// answered `invalid_or_unavailable_policy` 8,460 times out of 12,009 passes,
/// freed zero bytes across all of them, and the volume reached 100% with a
/// janitor running every minute the whole way down.
///
/// Prefer an in-place restart. A launchd definition whose program changed must
/// be reloaded in its observed owner domain; a kick would reuse the stale argv.
/// Both paths verify the resulting kernel image before delivery can succeed.
///
/// A failed reader refresh fails delivery even though the binary has already
/// been installed. Retrying delivery must finish the runtime half rather than
/// reporting success merely because the installed pathname is current.
///
/// The queue agent is deliberately left alone. `cli::release_cmd::install_local`
/// writes `~/.stado/bin/stado.release-version`, and
/// `providers::local::agent` compares that file with the version it was
/// compiled from, finishes the slot it is holding, and lets its supervisor
/// recreate it. Kicking it here would abort a running job to save it a few
/// minutes, so a unit whose argv carries the `agent` subcommand is reported
/// and skipped. That handshake is the only one any unit implements, which is
/// why every other unit needs this function at all.
///
/// `context` prefixes every line, because both delivery paths call this and a
/// log that says `self-update` about a `release install-local` is a lie.
///
/// [`replace_verified`]: super::replace::replace_verified
pub(crate) async fn recycle_replaced_units(
    context: &str,
    install_dir: &Path,
    replaced: &[String],
    log_fn: &mut dyn FnMut(&str),
) -> Result<(), String> {
    let paths: Vec<String> = replaced
        .iter()
        .map(|name| install_dir.join(name).to_string_lossy().into_owned())
        .collect();
    let ours = std::process::id();
    let restarted = if cfg!(target_os = "macos") {
        recycle_launchd(context, &paths, ours, log_fn).await?
    } else {
        recycle_systemd(context, &paths, ours, log_fn).await?
    };
    if restarted == 0 {
        log_fn(&format!(
            "{context}: no other managed unit was running a replaced binary"
        ));
    }
    Ok(())
}

/// Whether an argv belongs to the queue agent, which recycles itself through
/// the installed-release handshake and must not be kicked mid-slot.
///
/// Crate-visible because it is the one place this exclusion is written down.
/// `release_unit_image::revisit_plan` applies the same rule on a
/// schedule, and the fleet agent is one of the units that goes stale, so a
/// second spelling of "which units recycle themselves" is a second answer
/// waiting to disagree with this one.
pub(crate) fn defers_to_release_handshake<S: AsRef<str>>(argv: &[S]) -> bool {
    let mut arguments = argv.iter().skip(1).map(AsRef::as_ref);
    let first = arguments.next();
    let subcommand = if first == Some("--") {
        arguments.next()
    } else {
        first
    };
    subcommand == Some("agent")
}
