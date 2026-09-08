//! Stable JSON automation facade for Stado job lifecycle operations.
//!
//! Port of `stado/machine.py`. Every operation returns plain data; the CLI
//! layer (`cli/machine.rs`) wraps results in the versioned envelope
//! `{"schema_version":
//! 1,"ok":bool,"result"|"error":...}` serialized with
//! [`canonical_json`] (Python `json.dumps(..., ensure_ascii=False,
//! sort_keys=True, separators=(",", ":"))`).
//!
//! Errors are [`MachineError`] with a stable `code` (INVALID_REQUEST,
//! IDEMPOTENCY_CONFLICT, NOT_FOUND, INVALID_CURSOR, NOT_TERMINAL,
//! ARTIFACT_SECURITY, NO_ARTIFACTS, SERVICE_DIRECTORY_STALE, ...) and a
//! `retryable` flag, exactly the contract Python's `_invoke` emits.
//! Unexpected storage/IO/JSON failures map to code INTERNAL with
//! retryable=false (Python `_invoke`'s catch-all).
//!
//! # Compatibility policy
//!
//! Within `schema_version` 1 the envelope only grows: existing fields keep
//! their name, type, and meaning and are never removed; changes ship as new
//! optional fields, and a consumer must ignore fields it does not
//! recognize. Error codes are stable identifiers — a code is never reused
//! for a different condition. New codes may appear; a consumer must treat
//! an unknown code as a non-retryable failure unless the envelope's
//! `retryable` flag says otherwise. Any change that breaks these rules
//! increments [`SCHEMA_VERSION`]. The CLI emits exactly one schema version
//! per binary — there is no flag to request an older one — so a consumer
//! must refuse an envelope whose `schema_version` it does not know rather
//! than guess.

mod contract;
mod facade;
mod requests;
mod sources;

pub use contract::encoding::canonical_json;
pub use contract::error::MachineError;
pub use contract::jobs::{
    normalize_job, recorded_instance, RecordedInstance, JOB_PREFIXES, LOCAL_INSTANCE_PREFIX,
};
pub use contract::SCHEMA_VERSION;
pub use facade::MachineFacade;
pub use requests::validate::validate_request;
pub use sources::{MAX_SOURCE_ARCHIVE_BYTES, MAX_SOURCE_EXTRACTED_BYTES, MAX_SOURCE_MEMBERS};

pub(crate) use contract::encoding::utcnow;
