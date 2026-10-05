//! Binary self-update from an exact, immutable Stado release coordinate.
//!
//! The operator configures the public Stado release API together with one
//! exact version and platform. There is no mutable channel pointer, second
//! bucket to try, or provider credential path. Each requested object is
//! addressed as `stado://releases/stado/<version>/<platform>/<name>` through
//! `/api/release/object`.
//!
//! Remediation downloads the signed release's manifest (`release.json`) and
//! archive (`release.tar.gz`) into a temporary directory on the install
//! filesystem, checks the archive's size and digest against the manifest,
//! then atomically replaces `stado`. Any configuration, fetch, digest, or
//! filesystem failure aborts before the first rename.
//!
//! After a successful update the agent calls [`reexec`], replacing the process
//! image with the new binary while preserving argv and the environment.
//!
//! `reexec` covers the process that ran the update and nothing else. Every
//! OTHER long-running unit installed from the same directory keeps executing
//! the inode it started with, so [`recycle_replaced_units`] restarts those in
//! place once the new binaries are on disk.
//!
//! One stage per component: `release` reads the configured coordinate,
//! `verify` downloads and checks every byte, `swap` installs the result and
//! puts the fleet back on it, and `receipt` leaves the provenance copy the
//! fleet's attestation check reads.

mod receipt;
mod release;
mod swap;
mod verify;

// `cli::config_cmd`, `cli::release_cmd::local::install` and
// `providers::local::helpers` name `platform_triple_short`; `coordinator` and
// `providers::local::probe::version_check` name `self_update`,
// `UpdateOutcome` and `reexec`; `deploy::local_install::artifact` names
// `HttpReleaseFetcher`, `ReleaseFetcher` and `sha256_hex`. `SelfUpdateError`
// is the error of every re-exported signature here and `update_targets` is the
// published name for the replacement set, so both stay nameable at this path.
pub use release::binaries::{platform_triple_short, RELEASE_BINARIES};
pub use release::error::{SelfUpdateError, UpdateOutcome};
pub use release::fetcher::{HttpReleaseFetcher, ReleaseFetcher};
pub use swap::replace::reexec;
pub use verify::install::self_update;
pub use verify::sums::sha256_hex;
pub use verify::targets::update_targets;

// `cli::release_cmd::local::install` and `deploy::local_install::artifact`
// name `stage_for_attestation`; `cli::release_cmd::local::converge` and
// `cli::release_cmd::local::install` name `recycle_replaced_units`;
// `deploy::host_release::deliver::restart` and `release_unit_image::plan`
// name `defers_to_release_handshake`.
pub(crate) use receipt::stage_for_attestation;
pub(crate) use swap::recycle::{
    defers_to_release_handshake, recycle_replaced_units, replacement_in_flight, ReplacementInFlight,
};
