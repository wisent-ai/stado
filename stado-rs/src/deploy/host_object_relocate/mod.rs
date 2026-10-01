//! `stado space relocate TARGET` — move objects from one key prefix to another
//! INSIDE the store, on the declared host that holds it.
//!
//! The object API exposes GET, PUT, DELETE, list and stat and nothing else:
//! there is no move and no server-side copy. Re-addressing an object by
//! downloading the body to the control plane and uploading it back under the
//! other key pulls every byte through the loopback writer tunnel, retries, and
//! leaves a peer's publish answering 502 behind it — enough to take a host's
//! release ingress down for minutes. The bytes never need to move at all. Both
//! stores are directories on that machine, and a re-address inside one
//! directory tree is a `link` and an `unlink` — no network, no writer, no body
//! in flight.
//!
//! Adding a move route to the object API would have been the other answer. It
//! is the worse one: a new verb on a live store, reachable by anything holding
//! a bearer, to serve a defect's cleanup. This command uses the shared
//! registry-authorized host channel instead.
//!
//! The shape and the rules come from [`crate::deploy::host_disk`] via
//! [`crate::deploy::host_channel`]: one FIXED remote program, registry data
//! reaching ssh only as the destination, every value spliced through
//! [`shlex_quote`](super::shlex_quote), the tab-delimited `STADO_*` marker
//! protocol on the way back, and the report closed by
//! [`host_channel::finish_report`].
//!
//! What the remote program guarantees, because a relocation that loses an
//! object is worse than the mis-addressing it repairs:
//!
//! - **It refuses to overwrite.** A destination that already exists is never
//!   written. When its content hashes equal the source's, the pair is a
//!   half-finished earlier move and the source is dropped; when they differ,
//!   the object is reported [`DESTINATION_DIFFERS`] and BOTH copies are kept.
//! - **It verifies before it removes.** The destination is hard-linked into
//!   place, hashed from disk, and compared against the source's hash. Only
//!   then is the source unlinked. A mismatch unlinks the destination and keeps
//!   the source.
//! - **It is resumable.** Every outcome is a function of what is on disk, so a
//!   run that was interrupted, timed out, or bounded by `--limit` is finished
//!   by running it again. Nothing is recorded anywhere but the store itself.
//! - **It previews by default.** `--apply` is the only thing that changes a
//!   byte, the same way [`crate::deploy::host_reclaim`] is written.
//!
//! The metadata sidecar travels with the body. `LocalBackend` keeps it at
//! `.metadata/<path>.json` beside the store root and `delete` removes both, so
//! a body moved without its sidecar would leave `list --long` describing the
//! old address and the new object carrying nothing.
//!
//! Like [`crate::deploy::host_disk`]'s script the remote program is a raw
//! string: `\t` / `\n` inside it are the two literal characters the remote
//! `printf` expands, not Rust escapes.

use serde_json::Value;

use super::host_channel;
use super::{DeployError, Runner};

mod plan;
mod program;
mod receipts;
mod report;

pub use plan::{validate_prefix, RelocatePlan};
pub use program::remote_script;
pub use receipts::{
    is_refusal, parse_output, MetadataMove, RelocateReading, Relocation, CONVERGED,
    DESTINATION_DIFFERS, LINK_FAILED, MOVED, VERIFY_FAILED, WOULD_MOVE,
};
pub use report::to_report;

/// `status` for a run whose every object reached a decided outcome.
pub const OK_STATUS: &str = "ok";

/// The store root, relative to the remote login user's `$HOME`, when the
/// operator names none.
///
/// The same default `storage.local.path` carries
/// ([`crate::config::wc_local_storage_path`]), written relative because
/// `$HOME` expands only on the far side. It is the object API's own backing
/// directory on the always-on mac.
pub const DEFAULT_STORE_ROOT: &str = ".stado/local-storage";

/// Relocate one key prefix to another inside one canonical registry host's
/// store, or report what a pass would move.
pub async fn relocate_host(
    target_name: &str,
    plan: &RelocatePlan,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let RelocatePlan {
        namespace,
        from,
        to,
        store_root,
        apply,
        limit,
    } = plan;
    let (apply, limit) = (*apply, *limit);
    validate_prefix("--namespace", namespace)?;
    if namespace.is_empty() || namespace.contains('/') {
        return Err(DeployError(format!(
            "--namespace is one store namespace, e.g. probierz: {namespace}"
        )));
    }
    validate_prefix("--from-prefix", from)?;
    validate_prefix("--to-prefix", to)?;
    if from == to {
        return Err(DeployError(format!(
            "--from-prefix and --to-prefix name the same address, so there is nothing to move: {from}"
        )));
    }
    if from.is_empty() {
        return Err(DeployError(
            "--from-prefix would select the whole namespace; name the mis-addressed prefix"
                .to_string(),
        ));
    }
    let target = host_channel::canonical_target(target_name).await?;
    // `$HOME` expands only on the far side, so an unnamed root is composed
    // there rather than guessed from this machine's own configuration.
    let root = match store_root {
        Some(named) => named.to_string(),
        None => format!(
            "{}/{DEFAULT_STORE_ROOT}",
            host_channel::remote_home(&target, runner).await?
        ),
    };
    let script = remote_script(&root, namespace, from, to, apply, limit);
    let output = host_channel::run_script(&target, &script, runner).await?;
    let reading = parse_output(&output.stdout);
    let mut report = to_report(&target, &reading, namespace, apply);
    host_channel::finish_report(&mut report, &output, OK_STATUS, "ssh failed");
    Ok(Value::Object(report))
}
