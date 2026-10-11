//! The stops of `stado serve` a host keeps in `~/.stado/serve-stops.jsonl`,
//! read by both sides of a crash loop: `stado serve` appends each stop and
//! raises the Oko defect when a build stops again, and a source install
//! refuses to build a revision whose stops raised one.
//!
//! The second half is the hold. After `stado release local restore
//! --previous` puts a working build back, product sync's next pass would
//! build the canonical checkout again — the very revision that stopped on
//! every start — and the host would loop through the same failure. A
//! revision with a raised stop is held: the install is refused with the
//! stops and the defect, and the next commit is built as usual.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One stop of `stado serve`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stop {
    pub version: String,
    pub revision: String,
    pub at: String,
    pub failure: String,
    /// Whether this stop raised the Oko defect for its build.
    #[serde(default)]
    pub reported: bool,
    /// What raising it answered: the defect or Oko's refusal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
}

/// Where a host keeps its stops, under the home `home`.
pub fn path(home: &Path) -> PathBuf {
    home.join(".stado").join("serve-stops.jsonl")
}

/// Every stop kept at `path`; none when nothing was ever kept.
pub fn read(path: &Path) -> Result<Vec<Stop>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("{} cannot be read: {error}", path.display())),
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|error| {
                format!(
                    "{} holds a line that is not a stop ({error}): {line}",
                    path.display()
                )
            })
        })
        .collect()
}

/// Why `revision` is held from a source install on the host whose home is
/// `home`: the stops of a build of it, once one of them raised its Oko
/// defect. `None` when no build of it stopped again.
pub fn held(home: &Path, revision: &str) -> Result<Option<String>, String> {
    let kept = path(home);
    let stops: Vec<Stop> = read(&kept)?
        .into_iter()
        .filter(|stop| stop.revision == revision)
        .collect();
    let Some(raised) = stops.iter().find(|stop| stop.reported) else {
        return Ok(None);
    };
    let latest = stops.last().map(|stop| stop.failure.as_str());
    Ok(Some(format!(
        "revision {revision} is held: stado serve {} built from it stopped {} time(s) on this host ({}), the latest with: {}; {}. \
         A later commit is built as usual",
        raised.version,
        stops.len(),
        kept.display(),
        latest.into_iter().collect::<String>(),
        raised.report.as_deref().into_iter().collect::<String>(),
    )))
}
