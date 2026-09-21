//! What has been written but not yet proven: the fleet's delivery register.
//!
//! Writing code and building it are two different acts on two different
//! clocks. A session finishes a change in minutes and there are many sessions;
//! the fleet builds three times a day. While those two were welded together —
//! the only recorded way to run a suite was a build recipe whose command was
//! `cargo build && cargo test` — every session that wanted proof spent one of
//! the fleet's three builds, and on 2026-09-21 one session spent all three on
//! its own two crates and left the others with none.
//!
//! So a session now *delivers*: it records the product, the exact revision it
//! pushed, and the task that revision answers. The delivery costs nothing and
//! waits. A qualification pass later builds one head that carries a dozen
//! deliveries, runs the product's tests once, and writes the same verdict onto
//! every delivery it covered. A delivery that fails names its task, which is
//! how the work comes back to whoever wrote it instead of being lost.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::targets::{de_null_as_default, Registry};

/// Top-level registry key holding the delivery register. Unmodelled by
/// [`Registry`] itself, so the array round-trips through [`Registry::extra`]
/// and a writer of any vintage keeps it.
pub const DELIVERIES_KEY: &str = "deliveries";

/// Where one delivery stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    /// Written and pushed; no pass has covered it yet.
    Waiting,
    /// A qualification pass is building and testing a head that carries it.
    Qualifying,
    /// A pass covered it and the product's tests passed.
    Verified,
    /// A pass covered it and the product's tests failed. The task named by
    /// the delivery is the one that has to be reopened.
    Failed,
}

impl DeliveryState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Qualifying => "qualifying",
            Self::Verified => "verified",
            Self::Failed => "failed",
        }
    }

    /// Whether a pass may still pick this delivery up. A verified delivery is
    /// history; a failed one is somebody's open task, not a thing to rebuild.
    pub fn open(&self) -> bool {
        matches!(self, Self::Waiting | Self::Qualifying)
    }
}

/// One revision handed to the fleet for proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    /// Stable identifier, minted when the delivery is recorded.
    pub id: String,
    /// The product this revision belongs to — the name of the build recipe
    /// that knows how to build and test it.
    pub product: String,
    /// The repository the revision is in, so a reader needs nothing else to
    /// find it.
    pub repo: String,
    /// The exact full commit that was pushed.
    pub revision: String,
    /// One line saying what was delivered, in the words of whoever wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The task in Oko's register this revision answers, when there is one.
    /// A failed pass reopens exactly this task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// The session that delivered it, for the same reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub delivered_at: String,
    #[serde(default = "waiting_state")]
    pub state: DeliveryState,
    /// The pass that covered it, once one has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pass: Option<String>,
    /// When the verdict was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<String>,
    /// The sentence the pass gave: which platform failed and what the job
    /// said. Absent while waiting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Whether the failure has been handed back to its task. Written by
    /// whoever files the defect, so one failure reopens one task once.
    #[serde(default)]
    pub reported: bool,
}

fn waiting_state() -> DeliveryState {
    DeliveryState::Waiting
}

impl Delivery {
    /// The delivery a session records the moment it has pushed.
    pub fn new(
        id: impl Into<String>,
        product: impl Into<String>,
        repo: impl Into<String>,
        revision: impl Into<String>,
        delivered_at: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            product: product.into(),
            repo: repo.into(),
            revision: revision.into(),
            summary: None,
            task: None,
            session: None,
            delivered_at: delivered_at.into(),
            state: DeliveryState::Waiting,
            pass: None,
            settled_at: None,
            reason: None,
            reported: false,
        }
    }

    /// The short form a listing shows.
    pub fn short_revision(&self) -> &str {
        let cut = self.revision.char_indices().nth(SHORT_REVISION);
        match cut {
            Some((index, _)) => &self.revision[..index],
            None => &self.revision,
        }
    }
}

/// How much of a commit a listing prints. Git's own abbreviation length.
const SHORT_REVISION: usize = 8;

/// One qualification pass: one build of one head, and the deliveries it
/// answers for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QualificationPass {
    pub id: String,
    pub product: String,
    /// The head that was built and tested.
    pub revision: String,
    pub started_at: String,
    /// The build jobs this pass is waiting on, keyed by platform.
    #[serde(default, deserialize_with = "de_null_as_default")]
    pub jobs: BTreeMap<String, String>,
    /// The deliveries this pass answers for.
    #[serde(default, deserialize_with = "de_null_as_default")]
    pub deliveries: Vec<String>,
    /// `running`, `passed` or `failed`.
    #[serde(default = "running_status")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

fn running_status() -> String {
    QualificationPass::RUNNING.to_string()
}

impl QualificationPass {
    pub const RUNNING: &'static str = "running";
    pub const PASSED: &'static str = "passed";
    pub const FAILED: &'static str = "failed";

    pub fn open(&self) -> bool {
        self.status == Self::RUNNING
    }
}

/// Top-level registry key holding the qualification passes.
pub const PASSES_KEY: &str = "qualification_passes";

/// The register as the fleet holds it. An absent key and entries that do not
/// parse both yield nothing: one hand-edited entry must not hide the rest.
pub fn read_deliveries(registry: &Registry) -> Vec<Delivery> {
    read_entries(registry.extra.get(DELIVERIES_KEY))
}

/// Every qualification pass the fleet has recorded.
pub fn read_passes(registry: &Registry) -> Vec<QualificationPass> {
    read_entries(registry.extra.get(PASSES_KEY))
}

fn read_entries<T: serde::de::DeserializeOwned>(entry: Option<&Value>) -> Vec<T> {
    entry
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| serde_json::from_value(entry.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Replace the register. The key lives in [`Registry::extra`], so
/// [`Registry::to_document`] carries it into the canonical document like any
/// other unmodelled block.
pub fn write_deliveries(registry: &mut Registry, deliveries: &[Delivery]) {
    registry.extra.insert(
        DELIVERIES_KEY.to_string(),
        serde_json::to_value(deliveries).expect("deliveries serialize infallibly"),
    );
}

pub fn write_passes(registry: &mut Registry, passes: &[QualificationPass]) {
    registry.extra.insert(
        PASSES_KEY.to_string(),
        serde_json::to_value(passes).expect("passes serialize infallibly"),
    );
}
