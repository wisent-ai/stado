//! Interruption-safe reconciliation of the two fixed co-located local object roots.
//!
//! This is deliberately not `space relocate`: relocation moves one in-store
//! address and refuses overwrites. This transaction checkpoints both physical
//! roots with copy-on-write clones, then additively makes `local-storage`
//! contain `local-backup`'s exact objects and effective metadata. Backup bytes
//! and primary-only objects are never removed. The immutable full-primary
//! checkpoint retains conflicting primary bytes before the backup-winning
//! value is installed.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, BorrowedFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::time::{sleep, Instant};

use super::{host_channel, service};
use super::{shlex_quote, DeployError, Runner};
use base64::Engine;
use sha2::{Digest, Sha256};

mod activation;
mod entry;
mod finalisation;
mod preparation;

use activation::*;
use finalisation::*;
use preparation::*;

pub use entry::{reconcile_host, reconcile_host_worker};

pub const CHECKPOINT: &str = "checkpoint";
pub const APPLY: &str = "apply";
pub const FINALIZE: &str = "finalize";
pub const ACTIVATE: &str = "activate";
pub const ROLLBACK: &str = "rollback";
pub const STATUS: &str = "status";
pub const RUN: &str = "run";
pub const RESUME: &str = "resume";
const TIMEOUT: Duration = Duration::from_secs(60 * 60);
const PREFLIGHT: &str = "preflight";
const ARM_ACTIVATION: &str = "arm-activation";
const ARM_ROLLBACK: &str = "arm-rollback";
const RECORD_LIFECYCLE_DECISIONS: &str = "record-lifecycle-decisions";
static RESIDENT_OWNER_TOKEN: OnceLock<String> = OnceLock::new();
static RESIDENT_RUNNER_GATE: OnceLock<Value> = OnceLock::new();
static RESIDENT_LOCK_FD: OnceLock<i32> = OnceLock::new();
static RESIDENT_TARGET: OnceLock<crate::targets::ComputeTarget> = OnceLock::new();
static RESIDENT_NATIVE_MANAGER: OnceLock<Value> = OnceLock::new();

const REMOTE_PYTHON: &str = include_str!("../host_storage_reconcile.py");

const FENCE_SCHEMA: &str = "stado.storage-root-fence.v5";
const READ_FENCE: &str = "read-fence";
const READ_OWNER: &str = "read-owner";
const PREFLIGHT_EVIDENCE_FILE: &str = "preflight.json";
const CHECKPOINT_EVIDENCE_FILE: &str = "checkpoint-evidence.json";
const LIFECYCLE_DECISIONS_FILE: &str = "lifecycle-decisions.json";
const FINAL_LIFECYCLE_OBSERVATIONS_FILE: &str = "final-lifecycle-observations.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FileSnapshot {
    body_base64: String,
    sha256: String,
    mode: u32,
    uid: u32,
    gid: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PreparedScript {
    body: String,
    sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WriterFence {
    target: String,
    label: String,
    role: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    storage_evidence: Vec<String>,
    path: String,
    listener_port: Option<u16>,
    was_loaded: bool,
    was_runnable: bool,
    loaded_domains: Vec<String>,
    autostart: BTreeMap<String, bool>,
    prior_pid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prior_started_at: Option<String>,
    prior_loaded_environment: BTreeMap<String, String>,
    registry_declared_environment: BTreeMap<String, String>,
    unit_declared_environment: BTreeMap<String, String>,
    prior_executable: Option<String>,
    prior_sha256: Option<String>,
    prior_device: Option<u64>,
    prior_inode: Option<u64>,
    unit_snapshot: Option<FileSnapshot>,
    prior_native_state: Option<String>,
    prior_last_exit_code: Option<String>,
    prior_restart: Option<String>,
    prior_triggers: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    forward_object_recovery: Option<PreparedScript>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rollback_object_recovery: Option<PreparedScript>,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_pid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_started_at: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    restored_loaded_environment: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_executable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_device: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_inode: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_route: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QueueEffect {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_content: Option<String>,
    intended: crate::queue::control::QueueControl,
    #[serde(skip_serializing_if = "Option::is_none")]
    superseding: Option<crate::queue::control::QueueControl>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QueueFence {
    was_paused: bool,
    drained: bool,
    resumed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pause: Option<QueueEffect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    restoration: Option<QueueEffect>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LeaseAcquisition {
    subject_id: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    lease: Option<crate::autonomy::storage::PlacementLease>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    released_lease: Option<crate::autonomy::storage::PlacementLease>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ImmutableEvidenceReference {
    path: String,
    sha256: String,
    bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StorageRoots {
    primary: String,
    backup: String,
    prior_primary: String,
    prior_backup: Option<String>,
    runtime: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WriteFenceEffect {
    status: String,
    intent: Value,
    acquired_at: Option<i64>,
    released_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LifecycleFence {
    schema: String,
    transaction: String,
    resident_owner: Value,
    status: String,
    queue: QueueFence,
    writers: Vec<WriterFence>,
    transport_retained: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    non_storage_retained: Vec<Value>,
    staged_runtime: Option<super::host_release::StagedRelease>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    roots: Option<StorageRoots>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    write_fence: Option<WriteFenceEffect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    preflight_evidence: Option<ImmutableEvidenceReference>,
    #[serde(default)]
    rollback_preparation: bool,
    #[serde(default)]
    lease_acquisitions: Vec<LeaseAcquisition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repository_runner_gate: Option<Value>,
    prepared_at: i64,
    rechecked_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    activated_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    activation_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    restored_at: Option<i64>,
}

#[derive(Debug, Clone)]
struct ServiceCandidate {
    target: crate::targets::ComputeTarget,
    declared: service::ManagedService,
    loaded_domains: Vec<String>,
    observed_command: String,
    storage_evidence: BTreeSet<String>,
}

enum QueueEffectOutcome {
    Applied,
    Superseded(crate::queue::control::QueueControl),
}
