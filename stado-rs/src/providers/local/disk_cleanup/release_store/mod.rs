//! Retention for the immutable release objects a host's local store holds.
//!
//! Every `stado release submit` publishes a product version into
//! `ecosystem/releases/<product>/<version>/` of the canonical store, and the
//! objects are immutable by contract: nothing ever rewrites or deletes one
//! through the object API. On the host that carries the store's files that
//! contract had no counterpart. Measured on `charless-mac-mini` on
//! 2026-09-04: `local-storage/ecosystem/releases/stado` held 84 versions,
//! 21.6 GiB, while the disk sat under the janitor's 15 GiB low watermark with
//! every declared cleaner reporting zero — and the same release loop that
//! filled it kept publishing 0.6 GiB stado releases into it, each one failing
//! to land because the host would not claim work under disk pressure. The
//! loop that needed the disk was the loop that consumed it, and nothing in the
//! janitor could name what it was looking at.
//!
//! What a release version is still for, and therefore what this cleaner keeps:
//!
//! - a version a host is running, rolled back from, or cutting over to: named
//!   by `active`, `previous` or `candidate` in any `<state_dir>/<product>.json`
//!   this host's release agent writes;
//! - a version any host in the registry DECLARES, through
//!   `targets[].managed_versions`, and any version an operator pins in a
//!   config file on this host through `release.version`. These two were the
//!   gap. On 2026-09-04 `stado/0.15.21/darwin-arm64` was published complete,
//!   read successfully through the public release route at 21:21Z, and
//!   answered `{"state":"absent"}` for every one of its objects by 21:50Z:
//!   this cleaner deleted it under disk pressure because the object-API
//!   host's `~/.stado/release-state` was empty and nothing else named it.
//!   `install-stado.sh`, `self_update.rs` and the declared release host-state
//!   capability all pin a version by `STADO_RELEASE_VERSION` /
//!   `release.version`, and a declaration in the registry is the durable pin
//!   this cleaner reads. A version somebody has declared is not reclaimable
//!   scratch;
//! - a version a pipeline run still names, because a delivery job may fetch
//!   it: every run record under `runs/release-pipeline/` whose state is not
//!   terminal, and every run younger than the policy's `min_age_seconds`
//!   regardless of state, so a just-completed run can still be redelivered;
//! - the newest `keep_newest` versions of each product, ordered by version
//!   number, as the rollback ladder an operator can still reach through
//!   `stado release rollback`;
//! - the newest version that is actually INSTALLABLE — one carrying the full
//!   installer family for some platform: `<product>-v<version>-<platform>.tar.gz`,
//!   `release-manifest-<platform>.json` and `SHA256SUMS`. A newer coordinate
//!   is not a substitute for it. Every publisher claims
//!   `source-revision.json` create-only BEFORE any artifact
//!   (`release_control::RELEASE_REVISION_NAME`), so an interrupted publish
//!   leaves a version directory holding that one small file — and four such
//!   claims are exactly what filled the newest-three ladder on 2026-09-04
//!   while the last installable version fell off the bottom of it. A claim is
//!   not a release: counting one as the rollback ladder leaves a host with
//!   nothing to install and nothing to roll back to.
//!
//! Absence from these pins is not permission to delete. Reclaim also requires
//! a completed or reconciled pipeline run for the exact source revision held
//! in the version reservation. Unknown and failed publications remain retained;
//! installer-family publications are not covered by signed-pipeline evidence.
//! The report names these refusals instead of treating a missing run as proof
//! that a publisher stopped. Reclaim removes eligible payloads together while
//! retaining the immutable version and platform source reservations.
//!
//! Two things this cleaner refuses on purpose. It never touches a product that
//! has no state file and no run record on this host, because a store can hold
//! releases for a product this host does not serve and whose consumers it
//! cannot see; those are kept and reported as `product_not_served_here`. When
//! the policy omits `keep_newest`, the conservative default keeps the newest
//! three versions as a rollback ladder.
//!
//! Layout: [`inventory`] is the store inventory — the version directories a
//! product holds, the release families each one completes, and every pin that
//! still names a version (host state, registry declaration, config file,
//! pipeline run); [`decision`] is the keep-or-reclaim question, answered over
//! those names alone; [`reclaim`] is the reclamation itself and the pass that
//! spends the budget and writes the report. This module owns the names, the
//! report's skip keys and the inventory model all three share.

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

/// The release agent's state directory, relative to `$HOME`, when the policy
/// names none. Same default as the release target policy.
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

/// The signed manifest name of one platform's installable archive, as
/// `install-stado.sh`, `self_update.rs` and `deploy/host_release.rs` build it.
fn platform_manifest_name(platform: &str) -> String {
    format!("release-manifest-{platform}.json")
}

/// The archive name those same readers request.
fn platform_archive_name(product: &str, version: &str, platform: &str) -> String {
    format!("{product}-v{version}-{platform}.tar.gz")
}

/// The digest list published beside them.
const PLATFORM_SUMS_NAME: &str = "SHA256SUMS";

/// The signed pipeline's release, named from `release_control` rather than
/// spelled again here: the manifest, its signature and the archive. The
/// qualification receipt is deliberately not required - a release can be
/// deployed from these three, and demanding a fourth would make the pin
/// narrower than the thing it protects.
const SIGNED_FAMILY: [&str; 3] = [
    crate::release_control::RELEASE_MANIFEST_NAME,
    crate::release_control::RELEASE_SIGNATURE_NAME,
    crate::release_control::RELEASE_ARCHIVE_NAME,
];

/// How many newest versions per product survive with no other reason, when
/// the policy sets `keep_newest` without a number.
const DEFAULT_KEEP_NEWEST: usize = 3;

/// One product's versions on disk, with the bytes each one occupies and, per
/// version, which release families it completes (see
/// [`complete_families`](inventory::families::complete_families)).
#[derive(Debug, Default)]
struct ProductReleases {
    versions: BTreeMap<String, (PathBuf, i64)>,
    complete: BTreeMap<String, BTreeSet<&'static str>>,
}

#[derive(Default)]
struct RunRetentionEvidence {
    pinned: BTreeMap<String, BTreeSet<String>>,
    finished: BTreeMap<String, BTreeMap<String, BTreeSet<String>>>,
}

/// Whether one version directory carries a COMPLETE release for at least one
/// platform, and in which of the two families.
///
/// Two publishers write this prefix and their object names are disjoint by
/// design (`release_control::RELEASE_REVISION_NAME`'s documentation):
///
/// - the tag train writes the installer family - `<product>-v<version>-<platform>.tar.gz`,
///   `release-manifest-<platform>.json` and `SHA256SUMS` - which is what
///   `install-stado.sh` and `self_update.rs` fetch;
/// - the signed pipeline writes `release.json`, `release.sig` and
///   `release.tar.gz`, which is what `stado release submit` publishes and
///   `stado web deploy` and `deploy/host_release.rs` install from.
///
/// Only the first was recognised here, and `stado` is the one product that
/// has both. Every pipeline-signed product - `preferences-landing`,
/// `preferences`, and the roughly thirty-five web products behind them -
/// publishes only the second, so no version of any of them was ever
/// installable by that definition and none could ever hold the
/// newest-complete-release pin. Their newest version was protected by
/// `keep_newest` alone, which is a count of directories and not a statement
/// about whether any of them can be deployed.
///
/// All three names of a family, from ONE platform directory: two of them is
/// what an interrupted publish leaves, and both installers read a manifest
/// and then verify the archive against its digest, so a version missing
/// either fails after the download instead of before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReleaseFamily {
    /// The tag train's objects, read by `install-stado.sh`.
    Installer,
    /// The signed pipeline's objects, read by `stado web deploy` and
    /// `host release`.
    Signed,
}

/// The report's skip key for the newest complete release of one family. They
/// are separate keys because a host reads one family and not the other, and
/// an operator asking why a version survived is asking which installer still
/// needs it.
fn family_key(family: ReleaseFamily) -> &'static str {
    match family {
        ReleaseFamily::Installer => "newest_installable_kept",
        ReleaseFamily::Signed => "newest_signed_release_kept",
    }
}
