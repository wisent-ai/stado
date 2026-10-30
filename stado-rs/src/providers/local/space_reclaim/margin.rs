//! The room under the rule a pass leaves for the next build.
//!
//! A pass that stops the moment the volume is under the rule leaves it a
//! fraction of a GiB below: the first build that starts writes it back over,
//! and the host refuses placements (`disk_pressure_active`) until the next
//! pass. So a pass's second tier keeps going until the volume can take the
//! largest build this host made recently. That size is measured, not
//! declared: the largest Cargo build directory the pass measured under
//! Cargo's home (running builds included), and the largest one the removal
//! record says a pass took in the last [`RECENT_DAYS`] days. With neither,
//! the margin is zero and the rule alone decides.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::build_caches::CARGO_BUILD_DIRS;
use super::candidates::ledger;

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
