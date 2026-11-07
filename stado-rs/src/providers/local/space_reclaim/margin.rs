//! The room under the rule a pass leaves for the next build, and the room a
//! host needs free before it takes work.
//!
//! A pass that stops the moment the volume is under the rule leaves it a
//! fraction of a GiB below: the first build that starts writes it back over,
//! and the next pass finds the volume at the rule again. So a pass's second
//! tier keeps going until the volume can take the largest build this host
//! made recently. That size is measured, not declared: the largest Cargo
//! build directory the pass measured under Cargo's home (running builds
//! included), and the largest one the removal record says a pass took in the
//! last [`RECENT_DAYS`] days. With neither, the margin is zero and the rule
//! alone decides what a pass takes.
//!
//! The rule decides what the janitor deletes, never whether work fits:
//! [`Room`] is admission's measure. A host takes work while the free bytes of
//! the volume the fleet writes to are above the largest scratch tree a
//! release build on this host recorded in the last [`RECENT_DAYS`] days plus
//! the margin the janitor's latest pass reported, at any percentage used.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::build_caches::CARGO_BUILD_DIRS;
use super::candidates::ledger;
use super::rule::{VolumeReading, DISK_FULL_PERCENT};
use super::STATE_DIR_PARTS;
use crate::providers::local::host_memory::constants::GIB;

/// How far back the removal record is read for a build's size: a week of
/// this host's builds, so a workspace built every few days still counts and
/// one nobody has built for a month does not hold space for itself.
pub const RECENT_DAYS: i64 = 7;

/// Where the margin's size was measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A Cargo build directory this pass measured.
    ThisPass,
    /// A Cargo build directory the removal record says a recent pass took.
    RemovalRecord,
}

impl Source {
    fn word(self) -> &'static str {
        match self {
            Self::ThisPass => "measured_this_pass",
            Self::RemovalRecord => "removal_record",
        }
    }
}

/// Bytes the volume should still take under the rule when a pass ends, and
/// the build directory they were measured on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Margin {
    pub bytes: i64,
    pub measured_on: Option<(PathBuf, Source)>,
}

impl Margin {
    /// The larger of this pass's largest Cargo build directory and the
    /// largest one recorded removed in the last [`RECENT_DAYS`] days. The
    /// record is read from `state_dir`; when it cannot be read the margin
    /// stands on this pass's measurement and the reason is returned beside it.
    pub fn read(state_dir: &Path, this_pass: Option<&(PathBuf, i64)>) -> (Self, Option<String>) {
        let mut margin = match this_pass {
            Some((path, bytes)) => Self {
                bytes: *bytes,
                measured_on: Some((path.clone(), Source::ThisPass)),
            },
            None => Self::default(),
        };
        let rows = match ledger::newest(state_dir, usize::MAX) {
            Ok(rows) => rows,
            Err(cause) => {
                return (
                    margin,
                    Some(format!(
                        "the build margin stands on this pass's measurement alone: {cause}"
                    )),
                )
            }
        };
        let since = chrono::Utc::now() - chrono::Duration::days(RECENT_DAYS);
        for row in rows {
            let recent = row["removed_at"]
                .as_str()
                .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
                .is_some_and(|at| at >= since);
            if !recent || row["cleaner"] != CARGO_BUILD_DIRS {
                continue;
            }
            let (Some(bytes), Some(path)) = (row["bytes"].as_i64(), row["path"].as_str()) else {
                continue;
            };
            if bytes > margin.bytes {
                margin = Self {
                    bytes,
                    measured_on: Some((PathBuf::from(path), Source::RemovalRecord)),
                };
            }
        }
        (margin, None)
    }

    /// The `margin` object a pass's report carries.
    pub fn to_json(&self) -> Value {
        json!({
            "bytes": self.bytes,
            "measured_on": self.measured_on.as_ref().map(|(path, _)| path),
            "source": self.measured_on.as_ref().map(|(_, source)| source.word()),
            "recent_days": RECENT_DAYS,
        })
    }
}

/// The key a capacity publication's `diag` carries the agent's [`Room`]
/// under, or `{"unread": <cause>}` when it could not judge one.
pub const ROOM_KEY: &str = "disk_room";

/// Bytes as GiB for a sentence.
fn gib(bytes: i64) -> f64 {
    bytes as f64 / GIB as f64
}

/// What the volume the fleet writes to has free, and what work on this host
/// needs free there. Each figure names where it was measured, without a local
/// path: the room is published with the capacity diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Room {
    pub free_bytes: i64,
    /// The largest scratch tree a release build on this host recorded in the
    /// last [`RECENT_DAYS`] days; zero with none.
    pub scratch_bytes: i64,
    pub scratch_source: String,
    /// The janitor's margin as its latest completed pass reported it.
    pub margin_bytes: i64,
    pub margin_source: String,
}

impl Room {
    /// The room on a volume with `free_bytes` free, from the janitor's latest
    /// completed pass report and this host's release scratch record beside
    /// the janitor's state, or why it cannot be judged: no completed pass has
    /// reported a margin yet, the home is unknown, or the record is
    /// unreadable.
    pub fn judge(free_bytes: i64, janitor_report: &Value) -> Result<Self, String> {
        let margin = &janitor_report["margin"];
        let Some(margin_bytes) = margin["bytes"].as_i64() else {
            let outcome = match janitor_report["outcome"].as_str() {
                Some(outcome) => outcome,
                None => "no outcome stated",
            };
            return Err(format!(
                "the janitor's latest pass reported no margin (outcome {outcome}); the next pass \
                 that completes measures it"
            ));
        };
        let margin_source = match margin["source"].as_str() {
            Some(source) => format!("the largest Cargo build directory, {source}"),
            None => format!(
                "no Cargo build directory measured or removed in the last {RECENT_DAYS} days"
            ),
        };
        let home = crate::config_file::home()
            .map_err(|cause| format!("the home holding the scratch record is unknown: {cause}"))?;
        let state_dir = home.join(STATE_DIR_PARTS[0]).join(STATE_DIR_PARTS[1]);
        let since = chrono::Utc::now() - chrono::Duration::days(RECENT_DAYS);
        let (scratch_bytes, scratch_source) =
            match crate::release_pipeline::largest_on_host(&state_dir, since)? {
                Some(receipt) => (
                    i64::try_from(receipt.bytes).map_err(|error| {
                        format!(
                            "the recorded release scratch of {} {} states {} bytes, more than a \
                             volume can hold: {error}",
                            receipt.product, receipt.platform, receipt.bytes
                        )
                    })?,
                    format!(
                        "the {} {} build of run {}, measured {}",
                        receipt.product, receipt.platform, receipt.run_id, receipt.measured_at
                    ),
                ),
                None => (
                    i64::default(),
                    format!("no release build recorded here in the last {RECENT_DAYS} days"),
                ),
            };
        Ok(Self {
            free_bytes,
            scratch_bytes,
            scratch_source,
            margin_bytes,
            margin_source,
        })
    }

    /// The free bytes work here needs: the scratch plus the margin.
    pub fn need_bytes(&self) -> i64 {
        self.scratch_bytes.saturating_add(self.margin_bytes)
    }

    /// Whether the free bytes are above the need.
    pub fn fits(&self) -> bool {
        self.free_bytes > self.need_bytes()
    }

    /// Both figures and where each part of the need was measured.
    pub fn sentence(&self) -> String {
        format!(
            "{:.1} GiB free against {:.1} GiB needed: {:.1} GiB the largest release scratch \
             recorded here ({}) and {:.1} GiB the janitor's margin ({})",
            gib(self.free_bytes),
            gib(self.need_bytes()),
            gib(self.scratch_bytes),
            self.scratch_source,
            gib(self.margin_bytes),
            self.margin_source
        )
    }

    /// The [`ROOM_KEY`] object a capacity publication carries.
    pub fn to_json(&self) -> Value {
        json!({
            "fits": self.fits(),
            "free_bytes": self.free_bytes,
            "need_bytes": self.need_bytes(),
            "release_scratch_bytes": self.scratch_bytes,
            "release_scratch_source": self.scratch_source,
            "margin_bytes": self.margin_bytes,
            "margin_source": self.margin_source,
        })
    }

    /// A [`ROOM_KEY`] object read back, or why it holds no room: the agent's
    /// own cause when it could not judge one, or the field it lacks.
    pub fn from_json(room: &Value) -> Result<Self, String> {
        if let Some(cause) = room["unread"].as_str() {
            return Err(cause.to_string());
        }
        let number = |key: &str| {
            room[key]
                .as_i64()
                .ok_or_else(|| format!("its {ROOM_KEY} states no {key}"))
        };
        let text = |key: &str| {
            room[key]
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("its {ROOM_KEY} states no {key}"))
        };
        Ok(Self {
            free_bytes: number("free_bytes")?,
            scratch_bytes: number("release_scratch_bytes")?,
            scratch_source: text("release_scratch_source")?,
            margin_bytes: number("margin_bytes")?,
            margin_source: text("margin_source")?,
        })
    }

    /// The room a capacity publication's `diag` states, or why it states none.
    pub fn published(diag: &Value) -> Result<Self, String> {
        match diag.get(ROOM_KEY) {
            Some(room) => Self::from_json(room),
            None => Err(format!(
                "its agent publishes no {ROOM_KEY}: it runs a Stado from before admission judged \
                 the room work needs; install a current Stado there"
            )),
        }
    }
}

/// Admission's verdict on a volume: `None` when work fits, else the sentence
/// the refusal carries. Where the room cannot be judged, the janitor's rule
/// decides as it did before the room was measured, and the sentence says why.
pub fn admission(room: &Result<Room, String>, reading: VolumeReading) -> Option<String> {
    match room {
        Ok(room) if room.fits() => None,
        Ok(room) => Some(room.sentence()),
        Err(cause) if reading.full() => Some(format!(
            "the room work here needs could not be judged ({cause}), so the janitor's \
             {DISK_FULL_PERCENT}% rule decides: {:.1}% used, {:.1} GiB free",
            reading.used_percent(),
            gib(reading.free_bytes)
        )),
        Err(_) => None,
    }
}
