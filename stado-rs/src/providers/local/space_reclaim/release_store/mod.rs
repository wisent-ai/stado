//! The immutable release objects a host's local store holds.
//!
//! Every `stado release submit` publishes a product version into
//! `ecosystem/releases/<product>/<version>/` of the canonical store, and the
//! objects are immutable by contract: nothing ever rewrites or deletes one
//! through the object API. On the host that carries the store's files that
//! contract had no counterpart: a product's releases piled up to dozens of
//! versions and tens of GiB, and the same release loop that fills the store
//! keeps publishing into it.
//!
//! What a release version is still for, and therefore what this cleaner keeps:
//!
//! - a version a host is running, rolled back from, or cutting over to: named
//!   by `active`, `previous` or `candidate` in any `<state_dir>/<product>.json`
//!   this host's release agent writes;
//! - a version any host in the registry DECLARES, through
//!   `targets[].managed_versions`, and any version an operator pins in a
//!   config file on this host through `release.version`. These two were the
//!   gap: a version published complete and read successfully through the
//!   public release route can answer `{"state":"absent"}` for every one of
//!   its objects half an hour later, because this cleaner deleted it under
//!   disk pressure when the object-API host's `~/.stado/release-state` was
//!   empty and nothing else named it.
//!   `install-stado.sh`, `stado self-update` and the declared release
//!   host-state capability all pin a version by `STADO_RELEASE_VERSION` /
//!   `release.version`, and a declaration in the registry is the durable pin
//!   this cleaner reads. A version somebody has declared is not reclaimable
//!   scratch;
//! - a version a pipeline run still names, because a delivery job may fetch
//!   it: every run record under `runs/release-pipeline/` whose state is not
//!   terminal;
//! - the newest version that is actually INSTALLABLE — one carrying the
//!   complete signed release (`release.json`, `release.sig`,
//!   `release.tar.gz`) for some platform — because that is what a host
//!   joining the fleet installs. A newer coordinate is not a substitute for
//!   it. Every publisher claims `source-revision.json` create-only BEFORE any
//!   artifact (`release_control::RELEASE_REVISION_NAME`), so an interrupted
//!   publish leaves a version directory holding that one small file, and a
//!   claim is not a release.
//!
//! Absence from these pins is not permission to delete. Reclaim also requires
//! a completed or reconciled pipeline run for the exact source revision held
//! in the version reservation. Unknown and failed publications remain retained.
//! The report names these refusals instead of treating a missing run as proof
//! that a publisher stopped. Reclaim removes eligible payloads together while
//! retaining the immutable version and platform source reservations.
//!
//! One thing this cleaner refuses on purpose: it never touches a product that
//! has no state file and no run record on this host, because a store can hold
//! releases for a product this host does not serve and whose consumers it
//! cannot see; those are kept and reported as `product_not_served_here`.
//!
//! Layout: [`inventory`] is the store inventory — the version directories a
//! product holds, which of them hold a complete signed release, and every pin
//! that still names a version (host state, registry declaration, config file,
//! pipeline run); [`decision`] is the keep-or-reclaim question, answered over
//! those names alone; [`reclaim`] is the reclamation itself and the pass that
//! writes the report. This module owns the names, the report's skip keys and
//! the inventory model all three share.

mod decision;
mod inventory;
mod reclaim;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

pub use inventory::pins::declared_versions;
pub use reclaim::pass::scan_release_store;

pub const CLEANER: &str = "release_store";

/// Where the local store keeps release objects, relative to `$HOME`. Same
/// default as `storage.local.path` plus the object namespace root.
pub const RELEASES_ROOT: &str = ".stado/local-storage/ecosystem/releases";

/// Where release pipeline runs live inside the store, relative to the
/// namespace directory of the store's own product namespace.
const RUNS_PREFIX: &str = "runs/release-pipeline";

/// The release agent's state directory, relative to `$HOME`. Same default as
/// the release target policy.
pub const STATE_DIR: &str = ".stado/release-state";

/// Config files an operator's version pin can live in, relative to `$HOME`,
/// in the same order [`crate::config_file::CANDIDATES`] resolves them. The
/// pin is read from EVERY one of them rather than from the winning file:
/// this cleaner is not resolving configuration, it is asking whether any
/// declaration on this host still needs a version, and a shadowed file's
/// answer is as expensive to get wrong as the winner's.
const CONFIG_CANDIDATES: [&str; 3] = [
    ".config/stado/config.json",
    ".stado/config.json",
    "stado.config.json",
];

/// The dotted config key holding the exact release version an installer
/// consumes, as [`crate::config::stado_release_version`] reads it.
const CONFIG_VERSION_KEY: [&str; 2] = ["release", "version"];

/// The product a bare `release.version` pin belongs to. That key names no
/// product because Stado's own installers are its only readers.
const CONFIG_VERSION_PRODUCT: &str = "stado";

/// The signed pipeline's release, named from `release_control` rather than
/// spelled again here: the manifest, its signature and the archive. The
/// qualification receipt is deliberately not required - a release can be
/// deployed from these three, and demanding a fourth would make the pin
/// narrower than the thing it protects.
const SIGNED_RELEASE: [&str; 3] = [
    crate::release_control::RELEASE_MANIFEST_NAME,
    crate::release_control::RELEASE_SIGNATURE_NAME,
    crate::release_control::RELEASE_ARCHIVE_NAME,
];

/// One product's versions on disk, with the bytes each one occupies, and the
/// versions that hold a complete signed release (see
/// [`signed_release_complete`](inventory::signed::signed_release_complete)).
#[derive(Debug, Default)]
struct ProductReleases {
    versions: BTreeMap<String, (PathBuf, i64)>,
    complete: BTreeSet<String>,
}

#[derive(Default)]
struct RunRetentionEvidence {
    pinned: BTreeMap<String, BTreeSet<String>>,
    finished: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
}
