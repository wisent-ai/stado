//! How long one boundary validation attempt may run, and how often a closed
//! boundary may be revalidated inline.

use std::time::Duration;

use super::Boundary;

/// Path of the live boundary-budget override, relative to `$HOME`.
///
/// Owner-controlled state beside `skarbiec.vault.json` and the token files this
/// unit already reads out of `$HOME/.stado`. Its absence is the normal state.
pub const BOUNDARY_TIMEOUT_OVERRIDE_PATH: &str = ".stado/dashboard-boundary-timeout-seconds";

/// The override's current value, or nothing.
///
/// Read on every validation attempt on purpose: a budget that can only be
/// changed by restarting the process is not an override for a stalled process.
/// A missing file, an unreadable one, a non-numeric body and a zero all read as
/// "no override", so a typo cannot disable the boundary by setting the budget
/// to nothing.
fn file_override_seconds() -> Option<u64> {
    let home = std::env::var_os("HOME")?;
    let path = std::path::Path::new(&home).join(BOUNDARY_TIMEOUT_OVERRIDE_PATH);
    std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|seconds| *seconds > 0)
}

/// How long one boundary validation attempt may run, at startup and on an
/// inline recheck alike.
///
/// Each boundary reads every item its policy names, and each read is a gpg
/// decryption in the broker. Seventeen object namespaces against a real vault
/// with a cold gpg-agent exceeded the previous 15s and the object API then
/// answered 503 to the entire fleet until someone restarted it -- a cold
/// agent is a slow start, not a broken grant.
pub(crate) fn boundary_timeout(boundary: Boundary) -> Duration {
    // The override a stalled unit can actually be given, read at validation
    // time from a file rather than from the environment.
    //
    // `WC_DASHBOARD_BOUNDARY_TIMEOUT_SECONDS` below is honoured for a process
    // that was launched with it, and it stays. What it cannot do is help in the
    // situation it exists for: a boundary stalling in a RUNNING process. Rust
    // reads env at exec, so the only way to apply it was to restart the very
    // service whose stall was the problem — and on 2026-08-31 that service was
    // the object store a release was publishing through, and its boundaries
    // then recovered by themselves while a restart would have bought a fresh
    // cold gpg-agent and destroyed the evidence. An escape hatch that requires
    // restarting the thing it is escaping is decorative.
    //
    // A file is re-read on every attempt, so an operator raises the budget with
    // one `echo` and lowers it by deleting the file, with nothing cycled. It is
    // owner-controlled state beside the vault and the token files this unit
    // already reads from `$HOME/.stado`.
    if let Some(configured) = file_override_seconds() {
        return Duration::from_secs(configured);
    }
    if let Some(configured) = std::env::var("WC_DASHBOARD_BOUNDARY_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
    {
        return Duration::from_secs(configured);
    }
    // Per mapped item, not flat. Each verifier boundary reads its grant and
    // then ONE vault field per mapped item, strictly serially, because a
    // Skarbiec request decrypts and rewrites shared state and fanning out
    // caused resets (`skarbiec::validate::object`). So the work is linear in
    // the number of declarations, and a fixed 90 seconds is a budget that
    // stops being true as the fleet grows.
    //
    // On 2026-08-31 charless-mac-mini declared 17 object namespaces, 14
    // release publishers and 4 service deployers. Every boundary failed with
    // "validation did not settle within 90 seconds", every object route
    // answered 503, two `queue resume` attempts died on it, and no release
    // could publish — while the vault was up, listening, and answering. The
    // same lesson is already recorded one module over in
    // `doctor::object_auth_deadline`, which budgets this exact sweep per item;
    // this is that fix applied to the boundary the whole fleet reads through.
    let mapped = match boundary {
        Boundary::Object => {
            crate::config::object_api_namespaces().map_or(usize::MIN, |items| items.len())
        }
        Boundary::Release => {
            crate::config::release_api_publishers().map_or(usize::MIN, |items| items.len())
        }
        Boundary::Machine => {
            crate::config::machine_api_clients().map_or(usize::MIN, |items| items.len())
        }
        Boundary::Service => {
            crate::config::service_api_deployers().map_or(usize::MIN, |items| items.len())
        }
        Boundary::Registry => {
            crate::config::registry_api_clients().map_or(usize::MIN, |items| items.len())
        }
        // These read a fixed, small set of material rather than one item per
        // declaration, so they keep the flat allowance.
        Boundary::RateLimitVerifier | Boundary::RateLimitState | Boundary::Integration => {
            usize::MIN
        }
    };
    BOUNDARY_ITEM_ALLOWANCE + BOUNDARY_ITEM_ALLOWANCE * u32::try_from(mapped).unwrap_or(u32::MIN)
}

/// Allowance for one grant read plus one mapped item. The flat value this
/// replaces, kept as the unit, so a deployment that declares nothing sees the
/// budget it always had.
const BOUNDARY_ITEM_ALLOWANCE: Duration = Duration::from_secs(90);
/// boundary. Long enough that a fleet hammering a shut boundary produces one
/// vault sweep per cooldown rather than one per request, short enough that a
/// transient reset costs seconds of 503 instead of a privileged restart.
pub(crate) fn boundary_recheck_cooldown() -> Duration {
    Duration::from_secs(
        std::env::var("WC_DASHBOARD_BOUNDARY_RECHECK_SECONDS")
            .ok()
            .and_then(|value| value.trim().parse::<u64>().ok())
            .filter(|seconds| *seconds > 0)
            .unwrap_or(30),
    )
}
