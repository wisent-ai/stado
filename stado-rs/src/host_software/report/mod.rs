//! The newest software report one host has on file, and how old it is.
//!
//! The report itself is here. [`store`] is the other half: it persists a
//! report, replaces it, records a refusal in its place and reads the newest
//! one back out of the observation store.

mod store;

use serde_json::{json, Value};

use crate::observations::{self, Freshness, OBSERVED};

use super::HostSoftware;

pub use store::{load, load_in, record, record_refusal, reported_hosts};

/// The newest software report one host has on file, and how old it is.
#[derive(Debug, Clone)]
pub struct Report {
    pub host: String,
    /// One row per program the newest report listed.
    pub rows: Vec<HostSoftware>,
    /// Shell scripts the host carries alongside. Counted rather than rowed: the
    /// retired helper channel left 1393 of them in `$HOME/.stado/bin` on
    /// control-host against 28 programs, and a release pipeline produces
    /// none of them — rowing each as `unmanaged` would bury the twenty-eight
    /// answers the report exists to give.
    pub scripts: usize,
    /// How old the fleet's knowledge of this host's software is.
    /// [`Freshness::Never`] is the state that was invisible.
    pub freshness: Freshness,
}

impl Report {
    /// Nothing on file for this host. Kept apart from an empty report: a host
    /// that carries no programs answered, and one that never answered did not.
    pub fn never(host: &str) -> Self {
        Self {
            host: host.to_string(),
            rows: Vec::new(),
            scripts: usize::default(),
            freshness: Freshness::Never,
        }
    }

    /// The state word of the look itself: [`OBSERVED`] when the host answered,
    /// [`UNVERIFIED`](crate::observations::UNVERIFIED) when the look could not
    /// happen, `never` when none was ever taken, or a word from a newer writer
    /// carried through.
    pub fn state(&self) -> &str {
        match &self.freshness {
            Freshness::Fresh(row) | Freshness::Stale(row) => row.state.as_str(),
            Freshness::Never => "never",
        }
    }

    /// Why, in the reporter's or the channel's own words. Empty when the host
    /// answered cleanly.
    pub fn refusal(&self) -> &str {
        match &self.freshness {
            Freshness::Fresh(row) | Freshness::Stale(row) if row.state != OBSERVED => {
                row.detail.as_str()
            }
            _ => "",
        }
    }

    /// `just now`, `14m ago`, `stale (3h)` or `never`, in the one spelling every
    /// other freshness column in this tree uses.
    pub fn age(&self) -> String {
        observations::render(&self.freshness)
    }

    pub fn released(&self) -> usize {
        self.rows.iter().filter(|row| row.is_release()).count()
    }

    pub fn unmanaged(&self) -> usize {
        self.rows.iter().filter(|row| !row.is_release()).count()
    }

    pub fn find(&self, name: &str) -> Option<&HostSoftware> {
        self.rows.iter().find(|row| row.name == name)
    }

    /// The counts as one phrase, for a row that has one column to say them in.
    pub fn summary(&self) -> String {
        if matches!(self.freshness, Freshness::Never) {
            return "no report".to_string();
        }
        format!(
            "{} program(s), {} release, {} unmanaged, {} script(s)",
            self.rows.len(),
            self.released(),
            self.unmanaged(),
            self.scripts
        )
    }

    pub fn json(&self) -> Value {
        json!({
            "host": self.host,
            "state": self.state(),
            "observed": self.age(),
            "detail": self.refusal(),
            "reported": self.rows.len(),
            "release": self.released(),
            "unmanaged": self.unmanaged(),
            "scripts": self.scripts,
            "programs": self.rows.iter().map(HostSoftware::json).collect::<Vec<Value>>(),
        })
    }
}
