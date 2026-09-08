//! Release delivery for `stado release host-state --host TARGET --apply` — put
//! declared, managed binaries onto one registry host.
//!
//! NO Python original, and no Rust original either: `ARCHITECTURE.md` says
//! outright that "no system in the pack currently owns 'get this build onto
//! that host'. That is a gap". [`crate::deploy::host_inventory`] reads what a
//! host HAS, [`crate::targets::ComputeTarget`] declares what it SHOULD have,
//! and until this module nothing closed the two.
//!
//! It is not a new idea. Weles already ships the pattern this follows step
//! for step (`weles/scripts/worker/deploy/README.md`, "macOS worker +
//! auto-deploy"): fetch the canonical platform manifest and adjacent archive
//! through `/api/release/object`, verify the archive SHA-256, check the
//! required layout, stage the selected member under a versioned directory,
//! and only after every artifact is verified repoint the active release and
//! restart each declared unit that executes that install root. Independently
//! installed Stado reader trees consume the retained verified archive through
//! the enclosing `service converge` contract.
//! A missing or mismatched release archive leaves the currently active
//! release untouched and aborts the deployment. That
//! sentence is the whole design; everything below is it, applied to whatever
//! the fleet declares — one program, or one worker tree.
//!
//! What that buys, stated as contracts rather than intentions:
//!
//! - **`--binary` selects a declared product** ([`products`]). The
//!   declaration is one shipped document, `stado` and `weles-worker` are two
//!   entries in it, and the operator's word SELECTS an entry; it never
//!   becomes part of a path, a URI segment or a script word. This is
//!   [`crate::deploy::host_exec`]'s rule, kept: "the operator's words select
//!   a fixed argv entry and never join the command line". Every check below
//!   is as strict for the third entry as for the first: the same manifest
//!   identity, the same digest, the same platform agreement, the same
//!   proven-to-run version readback before anything is activated.
//! - **`--version` is an exact coordinate**, not a channel and not an
//!   alias. `latest` is a legal path segment, which is exactly why nothing
//!   here resolves one — see [`crate::release::canonical_coordinate`].
//! - **The digest comes from the canonical release manifest.** The control
//!   plane reads `release-manifest-<platform>.json` through the same Stado release API
//!   and storage contract that serves the artifact, validates its immutable
//!   product/version/platform/source-commit identity, and carries that digest
//!   to the host. Missing or malformed catalog data is a refusal.
//! - **Delivery executes a declaration; it does not replace one.** A host
//!   that declares no version for the binary, or declares a different one
//!   than `--version` names, is refused. Deciding what a host should run is
//!   the registry's job.
//! - **Verification strictly precedes activation, and that ordering is
//!   structural rather than careful.** The remote work is three separate
//!   programs on the shared channel — probe, stage, activate — and the
//!   activate program is only ever issued after the stage program reported
//!   a verified artifact. A failed fetch, a mismatched digest, a staged
//!   artefact that does not report the requested version: each leaves the
//!   running version untouched, because nothing has touched the install root
//!   yet. Splitting the phases is what makes the ordering observable at the
//!   [`Runner`] seam instead of buried inside one long shell script.
//! - **Activation is renames, never writes in place.** A program is
//!   hard-linked into a pending name beside the live one and renamed over it,
//!   so the active binary is the exact staged inode and there is no window in
//!   which `$HOME/.stado/bin/<name>` is half-written. It stays a REGULAR file
//!   on purpose: `host inventory` refuses to read through a symlink, so
//!   publishing a symlink here would blind the command that reports what is
//!   installed. A tree is replaced path by path out of the verified staging
//!   tree, one rename each, retiring the path it replaces.
//! - **A tree delivery replaces code and nothing else.** The install root of
//!   `weles-worker` is the artefact directory itself, which also holds
//!   `recordings/`, `var/` and `.work/` — host-local state no release
//!   produced. Those paths are declared
//!   ([`crate::deploy::products::Install::Tree`]), they are never named by an
//!   activation, and an artefact that carries one of them is refused at
//!   staging rather than allowed to overwrite it.
//!
//! What this command does NOT do, each for a reason:
//!
//! - it does not build, clone, fetch a tag, run a package manager or consult
//!   a channel pointer — Weles's auto-deploy does none of those either, and
//!   a host-side build is a host-side toolchain to keep alive;
//! - it does not choose a version, pick "the newest", or write the registry.
//!   Declaration is upstream of delivery on purpose: an automaton that
//!   deploys without knowing the intended state is a faster way to break
//!   production;
//! - it does not deliver more than one binary per invocation. Weles's
//!   auto-deploy stages a worker and two browsers together because they are
//!   one runtime; two independently versioned CLIs are not;
//! - it does not roll back. There is nothing to roll back from: a failure
//!   happens before activation, so the previous version remains active. A
//!   rollback changes `targets[].managed_versions` and applies host state,
//!   which is why the versioned staging tree is kept rather than pruned;
//! - it does not restart a unit it invented. A product declares every owning
//!   unit: a label alone has to be FOUND in the registry's own declared
//!   service set ([`service::declared_services`]) before it is touched, while
//!   a label together with the unit file locates a unit the product itself
//!   declares. Every resolved owner of this install root is restarted through
//!   the shipped `service restart` program. A product with no such units is
//!   activated and reported as having no units, not silently "restarted".

use std::time::Duration;

use serde_json::Value;

use super::products::{self, Product};
use super::{service, DeployError, Runner};
use crate::targets::ComputeTarget;

mod catalog;
mod coordinates;
mod deliver;
mod programs;

pub use coordinates::{
    is_exact_semver, is_sha256, plan, resolve_release_request, ReleasePlan, ReleaseRequest,
};
pub use deliver::{
    activate_staged_program, activate_staged_target, marker, marker_values, markers, release_target,
};
pub use programs::{
    activate_script, ensure_stado_reader_archive, probe_script, recheck_staged_script,
    stage_declared_release, stage_script, StagedRelease, FETCH_PRELUDE, REMOTE_ACTIVATE_BODY,
    REMOTE_PROBE_BODY, REMOTE_STAGE_BODY, SANITIZE_PRELUDE, TREE_ACTIVATE_BODY, TREE_DIR,
    TREE_PRELUDE, TREE_PROBE_BODY, TREE_STAGE_BODY,
};

pub(crate) use catalog::{catalog_identity, coordinate_revision_conflict, missing_release_objects};
pub(crate) use coordinates::loopback_http_origin;

/// `status` when the requested version was staged, verified and activated.
pub const RELEASED_STATUS: &str = "released";
/// `status` when the host already runs the requested version. Nothing was
/// fetched, staged, activated or restarted.
pub const ALREADY_ACTIVE_STATUS: &str = "already_active";
/// `status` for a `--dry-run`: the plan was built and the host was probed
/// read-only. No mutating program was sent.
pub const PLANNED_STATUS: &str = "planned";
/// A release archive can be hundreds of MiB. Keep the short host-channel
/// timeout for probes and activation, but bound download, hashing and extract
/// with enough time for the declared public release channel.
const STAGE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Verified Stado archive retained beside its attestation image until every
/// service-local reader has installed the same delivered bytes.
pub const READER_ARCHIVE_NAME: &str = "stado-reader-convergence.tar.gz";

/// The remote marker prefix, in the tab-delimited `STADO_*` protocol
/// [`crate::deploy::host_recovery::parse_output`] established.
pub const MARKER: &str = "STADO_RELEASE";

/// The registry key declaring, per binary name, the exact version a host is
/// supposed to be running. Named here only so a refusal can tell an
/// operator where to write the declaration.
pub const MANAGED_VERSIONS_KEY: &str = "managed_versions";

/// The immutable identity the canonical release manifest states for one
/// declared product at one coordinate: its source commit and the archive
/// digest a host must reproduce.
///
/// The manifest is the only source of both. A product whose declared version
/// was never published has no manifest, and this is where that is refused —
/// on the control plane, before a host is contacted, for a dry run exactly as
/// for a delivery.
///
/// A manifest is not enough on its own, which is why
/// [`missing_release_objects`] runs here too. The manifest is written early in
/// a publish, so it exists for versions whose binaries do not. Promotion and
/// host-state delivery both reach a coordinate through this one function.
/// Refusing here stops an incomplete immutable version from becoming desired
/// state or being delivered to a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CatalogIdentity {
    source_commit: String,
    sha256: String,
    archive_name: String,
    member: String,
}

/// The units whose executable is installed by this product on this host.
///
/// Two declarations can name each one, and both are declarations rather than
/// guesses. The registry's own service set wins whenever it carries the
/// label. For Stado, a registry unit executing
/// `~/.stado/services/<service>/current/.../stado` is deliberately excluded:
/// the root product installs only `$HOME/.stado/bin/stado`, so restarting that
/// private unit here would restart unchanged bytes and then compare them with
/// the new global digest. The shared private-reader convergence installs the
/// verified archive into those trees before it performs their lifecycle.
pub fn declared_units(target: &ComputeTarget, product: &Product) -> Vec<service::ManagedService> {
    let registry_units = service::declared_services(target);
    let mut resolved = Vec::new();
    for unit in &product.units {
        let label = unit.label_for(&target.name);
        let declared = registry_units
            .iter()
            .find(|candidate| candidate.matches(&label))
            .cloned()
            .or_else(|| {
                let path = unit.path_for(&target.name)?;
                Some(match unit.kind.as_deref()? {
                    products::UNIT_SYSTEMD => service::systemd_service(
                        &target.name,
                        &label,
                        &path,
                        service::SOURCE_PRODUCT,
                        "",
                    ),
                    _ => service::launchd_service(
                        &target.name,
                        &label,
                        &path,
                        service::SOURCE_PRODUCT,
                        "",
                    ),
                })
            });
        if let Some(declared) = declared {
            if product.name == "stado" && service::is_service_local_stado_reader(&declared) {
                continue;
            }
            if !resolved
                .iter()
                .any(|existing: &service::ManagedService| existing.unit_id() == declared.unit_id())
            {
                resolved.push(declared);
            }
        }
    }
    resolved
}

/// Deliver one declared product to one canonical registry host.
pub async fn release_host(
    target_name: &str,
    binary: &str,
    version: &str,
    dry_run: bool,
    reinstall: bool,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let (target, request, self_store) =
        resolve_release_request(target_name, binary, version, dry_run, reinstall, runner).await?;
    release_target(&target, &request, self_store, runner).await
}
