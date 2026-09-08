//! `stado registry validate|push|pull|self|doctor|host add|beacon-age` —
//! canonical registry management.
//!
//! `validate`, `push` and `pull` port the `registry` group of
//! `stado/cli.py`. `self`, `doctor`, `host add` and `beacon-age` have NO
//! Python original: they close items fifteen through seventeen of
//! `stado.wisent.com/docs/missing-commands`, written after the 2026-07-24
//! control-host incident, where the registry declared a host that
//! nothing on the box was honouring and no command could say so.
//!
//! Every read and write goes through [`targets::RegistryStore`], so the
//! group repairs the registry on whichever store `WC_STORAGE_BACKEND`
//! selects. It used to hardcode `gs://wisent-compute/registry.json` and
//! build a `GcsBackend` directly, which on an Azure-only deployment meant
//! the one document the coordinator's survival check reads
//! (`targets::fetch_registry_remote`) could be repaired by nobody.
//!
//! [`doctor`](fn@doctor) and [`beacon_age`] source liveness from the host beacons
//! (`monitor/host_health.rs`) and the capacity broadcasts
//! (`queue/capacity.rs`), never from ssh, so they cost one prefix listing
//! and are safe to run on a loop.
//!
//! `push` compare-and-swaps, but until `--if-generation` existed it could
//! only swap against the generation it had just read itself, which is no
//! condition at all: a file carries no provenance, so a document edited
//! against generation 9 and pushed after somebody published 10 landed on top
//! of 10 with every guard here satisfied. The token can now come from the
//! caller — `pull --generation-only` hands it out, `push --if-generation`
//! spends it — so the read the operator's edit was made against is the read
//! the write is conditional on. A refused write is
//! [`REGISTRY_CONFLICT_EXIT`], never the generic failure code, so a reconcile
//! loop can re-read and re-apply instead of forcing.
//!
//! One component tree per verb family: [`write`] for the conditional-write
//! helpers every mutation shares, [`commands`] for `validate`, `import`,
//! `push`, `pull`, `self` and the `host` family, [`beacons`] for the live
//! host state and `beacon-age`, and [`doctor`] for the divergence report.
//! Every name this module exposed before the split is re-exported here, so
//! `crate::cli::registry::NAME` still resolves.

mod beacons;
mod commands;
mod doctor;
mod write;

pub use crate::cli::registry::beacons::age::beacon_age;
pub use crate::cli::registry::commands::host::add::host_add;
pub use crate::cli::registry::commands::host::path::list::host_path_list;
pub use crate::cli::registry::commands::host::path::remove::host_path_remove;
pub use crate::cli::registry::commands::host::path::set::host_path_set;
pub use crate::cli::registry::commands::pull::pull;
pub use crate::cli::registry::commands::push::push;
pub use crate::cli::registry::commands::self_target;
pub use crate::cli::registry::commands::validate::import;
pub use crate::cli::registry::commands::validate::validate;
pub use crate::cli::registry::doctor::doctor;
pub use crate::cli::registry::write::conflict::REGISTRY_CONFLICT_EXIT;
pub use crate::cli::registry::write::document::commit_document;
pub use crate::cli::registry::write::document::fetch_document;
pub use crate::cli::registry::write::document::fetch_versioned_document;
pub use crate::cli::registry::write::document::push_document_if;

pub(crate) use crate::cli::registry::beacons::age::human_age;
pub(crate) use crate::cli::registry::doctor::capability::load_capability_measurements;
pub(crate) use crate::cli::registry::doctor::capability::Measurement;

use serde_json::Value;

use crate::cli::CmdError;
use crate::targets::{self, Registry};

/// The canonical registry for a read-only command, the last-known-good copy
/// when the authority does not answer, or the reason neither could be READ.
///
/// Never an empty registry: `doctor` reporting "every host is unmanaged"
/// because the store was down is the exact confusion
/// `targets::RegistryFetchError` exists to prevent. Never a silent copy
/// either — the copy's age goes to stderr in one sentence, because a
/// diagnostic that dies with the thing it diagnoses is worthless, and a
/// diagnostic that answers from a copy without saying so is worse.
pub(crate) async fn read_registry() -> Result<Registry, CmdError> {
    let (registry, notice) = targets::fetch_registry_or_last_good()
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    if let Some(notice) = notice {
        targets::report_registry_notice(&notice);
    }
    Ok(registry)
}

/// Python `click.echo(json.dumps(payload, indent=2, sort_keys=True))`.
fn echo_json(value: &Value) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(exc) => eprintln!("could not render json: {exc}"),
    }
}
