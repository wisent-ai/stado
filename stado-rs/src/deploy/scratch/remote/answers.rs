//! The answers those programs print, parsed into the values callers read.

use serde_json::Value;

use super::programs::STATUS_PREFIX;
use crate::deploy::scratch::lease::ScratchLease;

/// One lease as the host reports it: the record, whether its account is still
/// there, and where that account's home is. A record that cannot be parsed is
/// kept as unreadable rather than dropped, because an unreadable record is a
/// leak the reaper must still be able to destroy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostLease {
    pub name: String,
    pub account_present: bool,
    pub home_path: Option<String>,
    pub lease: Option<ScratchLease>,
    pub unreadable: Option<String>,
}

/// The field value a program prints when it has nothing to report, kept out of
/// the parsed value so a caller never renders a dash as a path.
const NOTHING: &str = "-";

/// Parse the `lease` markers out of the list program's answer.
pub fn parse_leases(stdout: &str) -> Vec<HostLease> {
    let mut rows = Vec::new();
    for line in stdout.lines() {
        let fields = crate::deploy::host_channel::marker_fields(line);
        let [prefix, "lease", account, home, name, body @ ..] = fields.as_slice() else {
            continue;
        };
        if *prefix != STATUS_PREFIX {
            continue;
        }
        let text = body.join("\t");
        let (lease, unreadable) = match serde_json::from_str::<ScratchLease>(text.trim()) {
            Ok(parsed) if parsed.schema == crate::deploy::scratch::lease::RECORD_SCHEMA => {
                (Some(parsed), None)
            }
            Ok(parsed) => (
                None,
                Some(format!("record declares schema '{}'", parsed.schema)),
            ),
            Err(exc) => (None, Some(exc.to_string())),
        };
        rows.push(HostLease {
            name: (*name).to_string(),
            account_present: *account == "present",
            home_path: reported(home),
            lease,
            unreadable,
        });
    }
    rows
}

/// A field the program filled in, or nothing.
fn reported(field: &str) -> Option<String> {
    if field.is_empty() || field == NOTHING {
        return None;
    }
    Some(field.to_string())
}

/// What the host said one name's account, home and record are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostState {
    pub account: String,
    pub home: String,
    pub record: String,
    /// The home directory the probe actually found, when it found one.
    pub home_path: Option<String>,
}

impl HostState {
    /// Everything the operation promised to remove is gone.
    pub fn is_clear(&self) -> bool {
        self.account == "absent" && self.home == "absent" && self.record == "absent"
    }

    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "account": self.account,
            "home": self.home,
            "record": self.record,
            "home_path": self.home_path,
        })
    }
}

/// Parse the `state` marker, refusing to invent a verdict when the probe
/// printed none: "the host says the account is gone" and "nobody asked the
/// host" are different facts.
pub fn parse_state(stdout: &str) -> Option<HostState> {
    stdout.lines().rev().find_map(|line| {
        let fields = crate::deploy::host_channel::marker_fields(line);
        let [prefix, "state", account, home, record, home_path] = fields.as_slice() else {
            return None;
        };
        if *prefix != STATUS_PREFIX {
            return None;
        }
        Some(HostState {
            account: (*account).to_string(),
            home: (*home).to_string(),
            record: (*record).to_string(),
            home_path: reported(home_path),
        })
    })
}

/// The value a single-marker program reported, by verb.
pub fn parse_marker(stdout: &str, verb: &str) -> Option<String> {
    stdout.lines().rev().find_map(|line| {
        let fields = crate::deploy::host_channel::marker_fields(line);
        let [prefix, printed, value @ ..] = fields.as_slice() else {
            return None;
        };
        if *prefix != STATUS_PREFIX || *printed != verb {
            return None;
        }
        Some(value.join("\t"))
    })
}
