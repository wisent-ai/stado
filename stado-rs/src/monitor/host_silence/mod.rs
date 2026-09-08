//! Durable record of a host going quiet, and of the readers that refused
//! while it was quiet.
//!
//! NO Python original. The incident it exists for: on 2026-08-19 between
//! 18:29 and 18:35 UTC `control-host` dropped off the tailnet — 100%
//! ping loss, ssh timing out, then `direct 10.0.0.253:41641` again with
//! 13-215 ms. Six minutes of a production host being unreachable, and
//! afterwards the product could not say it had happened. The beacon prefix
//! only ever holds the LATEST document per host, so the gap closed over
//! itself the moment the host came back: `host_health/<host>.json` was
//! fresh again and nothing anywhere remembered that it had been stale. The
//! only evidence that survived was an operator's two ping packets in a
//! terminal.
//!
//! The readers knew. The resolver refused resolutions with "service
//! directory cache is stale (store generation ...)" and its registry read
//! failed with "registry authority exited with ...: ssh: connect to host
//! ... Operation timed out" — both true, both timestamped, both written to
//! `~/.stado/logs/stado-resolver.err` and read by nobody. A refusal that
//! only a log file knows about is a refusal the product did not make.
//!
//! So two blob families, both append-only, both keyed by host:
//!
//! - `state/host_silence/<host>/<started_at>.json` — one record per gap,
//!   opened when the newest beacon crosses [`silence_threshold_seconds`]
//!   and closed by the first fresher beacon. `started_at` is the last
//!   moment the host was heard from, not the moment somebody noticed, so
//!   the duration is the outage rather than the polling interval.
//! - `state/reader_refusals/<host>/<at>.json` — one record per refusal,
//!   carrying the refusing component's own sentence VERBATIM in `detail`. A
//!   reader that rephrases the sentence it logged has invented a second
//!   vocabulary for one condition, and the operator then greps for a string
//!   that exists in no source file.
//!
//! Both live under `state/` and not at the store root; [`SILENCE_PREFIX`]
//! records why.
//!
//! `<host>` is the subject of the refusal, not the machine that refused:
//! the resolver on the laptop failing to reach the authority on the Mac
//! mini is evidence about the Mac mini, and it has to land where
//! `stado host link control-host` will look for it.
//!
//! The joins and transitions are pure functions over already-loaded
//! documents ([`beacon_is_silent`], [`open_record`], [`merge_observation`],
//! [`close_record`], [`summarize_refusals`]) so the truth table is
//! exercisable without a store, a network, or a sick host.
//!
//! The components are the seams this account already had: `records` holds
//! the three stored documents, `paths` the blob keys they are written
//! under, `transitions` the pure joins named just above, and `store` the
//! three things that need a `JobStorage` — reading the two families back,
//! the observer's open/close write, and best-effort refusal publication.
//! The vocabulary all four of them share — the two prefixes, the threshold
//! and its environment override, the reason and reader tokens — stays
//! here. Every name a caller outside this module uses is re-exported here,
//! so `crate::monitor::host_silence::<item>` resolves exactly as before.

mod paths;
mod records;
mod store;
mod transitions;

pub use paths::{refusal_object_path, refusal_prefix, silence_object_path, silence_prefix};
pub use records::{RefusalRecord, RefusalSummary, SilenceRecord};
pub use store::{
    observe_beacon_age, observe_beacon_age_at, open_silence, recent_refusals, recent_refusals_at,
    recent_silences, record_refusal, refusal_summary, refusal_summary_at, report_refusal,
    report_refusal_detached,
};
pub use transitions::{
    beacon_is_silent, close_record, merge_observation, open_record, silence_threshold_seconds,
    summarize_refusals,
};

/// Blob prefix holding one record per silence, per host.
///
/// `state/host_silence/`, NOT `host_silence/` at the store root, which is
/// where the first cut of this module wrote. The object API authorizes a
/// write by matching its key against this deployment's namespace prefix
/// allowlist; `host_silence/` and `reader_refusals/` are not in it, and
/// every write came back
/// `401 {"error":"unauthorized or non-immutable release write"}` — a
/// sentence naming neither the prefix nor the grant. Nothing was recorded
/// and `stado host link` printed a confident `"silences": []`, which is
/// strictly worse than printing nothing: it answers the question this
/// module exists for, wrongly.
///
/// `state/` satisfies both constraints at once. It is authorized wherever
/// the queue prefixes are, because the allowlist mirrors
/// [`crate::queue::copy::CANONICAL_PREFIXES`] — and it IS one of those
/// canonical prefixes, so a backend migration or a disaster-recovery
/// backup carries these records along with the queue state they explain.
///
/// The obvious alternatives are all taken. `host_health/` is walked
/// key-by-key by `registry doctor`'s beacon loader, which would report
/// every silence record as a phantom beacon of a host that does not exist;
/// `operations/` is walked the same way by the resource journal;
/// `diagnostics/` is authorized but deliberately OUTSIDE
/// `CANONICAL_PREFIXES` so that a cutover drops it, which is the wrong
/// home for the only surviving account of an outage.
pub const SILENCE_PREFIX: &str = "state/host_silence";

/// Blob prefix holding one record per reader refusal, per host. Rooted
/// under `state/` for the reason [`SILENCE_PREFIX`] gives.
pub const REFUSAL_PREFIX: &str = "state/reader_refusals";

/// Seconds of beacon age that open a silence when no operator overrides
/// `STADO_SILENCE_THRESHOLD_SECONDS`.
///
/// Five minutes, because the fleet's beacons are published on a one-minute
/// timer: three consecutive misses is a host that has stopped talking, one
/// miss is a slow `pmset` call.
pub const DEFAULT_SILENCE_THRESHOLD_SECONDS: i64 = 300;

/// Environment override for [`silence_threshold_seconds`].
pub const SILENCE_THRESHOLD_ENV: &str = "STADO_SILENCE_THRESHOLD_SECONDS";

/// The resolver's cached service directory aged past `max_stale` and it
/// stopped answering resolutions. Its own sentence: "service directory
/// cache is stale (store generation ...)".
pub const REASON_DIRECTORY_CACHE_STALE: &str = "directory_cache_stale";

/// A registry read through the service-directory authority failed at the
/// transport. Its own sentence: "registry authority exited with ...".
pub const REASON_AUTHORITY_UNREACHABLE: &str = "authority_unreachable";

/// A reader found the newest beacon for a host older than the silence
/// threshold and refused to answer from it.
pub const REASON_BEACON_STALE: &str = "beacon_stale";

/// `reader` values, the three components that read fleet state.
pub const READER_RESOLVER: &str = "resolver";
/// See [`READER_RESOLVER`].
pub const READER_CLI: &str = "cli";
/// See [`READER_RESOLVER`].
pub const READER_DASHBOARD: &str = "dashboard";
