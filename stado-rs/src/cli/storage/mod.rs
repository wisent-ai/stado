//! `stado storage` — the cross-backend copier plus the read-only
//! inspection commands the operator needs when the STORE itself is the
//! suspect.
//!
//! NO Python original: the Python CLI has neither a cross-backend copier
//! (see the module docs of [`crate::queue::copy`] for why) nor any way to
//! look at the raw store. This is the operator surface for both.
//!
//! `copy` and `verify` take every locator as an explicit flag, so the
//! source and the destination are built independently of
//! `WC_STORAGE_BACKEND` and can be two different kinds of store in the
//! same process. `ls` / `stat` / `cat` inspect the ONE configured store
//! and therefore go through [`crate::queue::JobStorage`].
//!
//! # Absent is not unreachable
//!
//! The GCP-billing outage left nobody able to answer "is the queue empty,
//! or is the store gone?", because
//! `<AzureBlobBackend as BlobBackend>::exists` maps EVERY failure to
//! `false` (Python parity: `except Exception: return False`) and
//! `<AzureBlobBackend as BlobBackend>::updated_at` maps every failure to
//! `None`. Through those two methods a forbidden container and an empty
//! one are the same answer. Nothing in this module calls either of them:
//!
//! - `stat` probes with `BlobBackend::download_text_versioned`, which
//!   propagates [`crate::queue::StorageError`]. `absent` (the store
//!   answered, the object is not there) and `unreachable` (the store did
//!   not answer) are different states with different exit codes.
//! - `ls` renders a per-prefix listing failure as `unreachable` and exits
//!   non-zero, instead of folding it into a zero count.
//! - `verify` treats an unreadable side as unknown, never as empty.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::num::NonZeroUsize;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use clap::{Args, Subcommand};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::queue::copy::{
    self, CopyOptions, CopyPlan, CopyReport, Endpoint, Outcome, CANONICAL_PREFIXES,
};
use crate::queue::{BlobBackend, BlobInfo, JobStorage, StorageError};
use crate::remote::object_store::OBJECT_API_CHUNK_BYTES;

use super::CmdError;
use crate::cli::reporting::table::print as print_table;

mod command;
mod inspect;
mod product;
mod transfer;

// One namespace for the components this file was split into. Every module
// below imports `crate::cli::storage::*`, so a name that used to sit beside
// its callers in one file still reaches them, and this block is the whole
// inventory of what crosses a component boundary.
pub use self::command::commands::{dispatch, StorageCommands};
pub use self::command::endpoint::EndpointArgs;
pub use self::inspect::cat::StorageCatArgs;
pub use self::inspect::ls::StorageLsArgs;
pub use self::inspect::stat::command::StorageStatArgs;
pub use self::product::store::object::StoragePutArgs;
pub use self::product::verbs::abort_upload::StorageAbortUploadArgs;
pub use self::product::verbs::get::{StorageGetArgs, StorageObjectsArgs};
pub use self::product::verbs::rm::{StorageRmArgs, StorageUrlArgs};
pub use self::transfer::archive::StorageArchiveArgs;
pub use self::transfer::copy::{StorageBackupArgs, StorageCopyArgs};
pub use self::transfer::verify::StorageVerifyArgs;

pub(crate) use self::product::endpoint::client::fleet_https_client;
pub(crate) use self::product::endpoint::origin::release_api_origin;
pub(crate) use self::product::endpoint::route::object_api_endpoint;
pub(crate) use self::product::store::fetch::{
    compare_and_swap_object, fetch_object, fetch_object_from_writer, fetch_object_versioned,
    list_object_uris,
};
pub(crate) use self::product::store::object::{store_object, store_object_with_metadata};
pub(crate) use self::product::store::release::claim::release_claim_source;
pub(crate) use self::product::store::release::coordinates::{
    published_release_coordinates, PublishedCoordinate,
};
pub(crate) use self::product::store::release::present::{
    release_object_present, release_object_size,
};
pub(crate) use self::transfer::copy::copy_between;
pub(crate) use self::transfer::verify::verify_between;

use self::inspect::cat::cat;
use self::inspect::ls::canonical::ls_canonical;
use self::inspect::ls::ls;
use self::inspect::ls::prefix::ls_prefix;
use self::inspect::stat::command::stat;
use self::inspect::stat::hint::inferred_namespace_hint;
use self::inspect::stat::presence::{unanswered_for_error, unanswered_for_status, Presence};
use self::inspect::stat::probe::probe;
use self::inspect::{backend_key, backend_prefix};
use self::product::api::{
    max_object_api_download_body, max_object_api_error_body, max_object_api_json_body,
    partial_content_bounds, RemoteComposeChunk, RemoteComposeRequest, RemoteComposeResponse,
    RemoteDeleteResponse, RemoteObjectApi, RemoteObjectAuth, RemoteObjectListResponse,
    RemotePutResponse,
};
use self::product::endpoint::origin::{configured_api_origin, configured_object_base_url};
use self::product::endpoint::route::response_body_detail;
use self::product::store::object::put;
use self::product::verbs::abort_upload::abort_upload;
use self::product::verbs::get::{get, objects};
use self::product::verbs::rm::{object_url, rm};
use self::transfer::archive::archive;
use self::transfer::copy::{backup, run};
use self::transfer::verify::diff::diff_prefix;
use self::transfer::verify::report::{diff_json, print_diff_detail, print_diff_table};
use self::transfer::verify::{verify, PrefixDiff};

fn parse_storage_kind(raw: &str) -> Result<String, String> {
    crate::capabilities::configurable_variant(crate::capabilities::RuntimeFacet::Storage, raw)
        .map(|variant| variant.id.to_string())
        .ok_or_else(|| {
            let choices =
                crate::capabilities::configurable_ids(crate::capabilities::RuntimeFacet::Storage)
                    .collect::<Vec<_>>()
                    .join(", ");
            format!("unknown storage backend {raw:?}; use one of: {choices}")
        })
}

/// [`copy::DEFAULT_CONCURRENCY`] as the non-zero type the flag parses into;
/// `buffered(0)` would make no progress, so zero is rejected at parse time.
fn default_concurrency() -> NonZeroUsize {
    NonZeroUsize::new(copy::DEFAULT_CONCURRENCY).expect("the crate fan-out budget is non-zero")
}

/// Default `--limit` for `storage ls`: the largest count one byte can
/// express. A hot prefix (`queue/` carries five figures of blobs) is
/// truncated with a note rather than flooding the terminal, and the
/// operator raises the flag when they want the rest.
fn default_list_limit() -> usize {
    usize::from(u8::MAX)
}

// ---- shared rendering ----

/// Pretty-printed JSON on stdout, same shape as `cli/quota.rs::echo_json`.
/// No Python original to match: none of these commands exist there.
fn echo_json(value: &Value) -> Result<(), CmdError> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn render_stamp(updated: Option<DateTime<Utc>>) -> String {
    updated.map_or_else(String::new, |stamp| {
        stamp.to_rfc3339_opts(SecondsFormat::Secs, true)
    })
}

fn render_optional_stamp(updated: Option<DateTime<Utc>>) -> Value {
    updated.map_or(Value::Null, |stamp| {
        json!(stamp.to_rfc3339_opts(SecondsFormat::Secs, true))
    })
}

fn render_metadata(metadata: &BTreeMap<String, String>) -> String {
    metadata
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<String>>()
        .join(",")
}

/// An unknown count renders as `?`, never as zero.
fn render_count(count: Option<usize>) -> String {
    count.map_or_else(|| "?".to_string(), |count| count.to_string())
}
