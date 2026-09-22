//! The incident this area is written from, as a state directory the agent
//! would recognise: the digest, the two refusals, and the audit trail.

use std::path::{Path, PathBuf};

use chrono::{Duration, Utc};
use serde_json::{json, Value};
use stado::release_agent::{
    parse_state_document, quarantine_audit_path, HostReleaseState, AGENT_ACTOR, STATE_SCHEMA,
};

pub(crate) const PRODUCT: &str = "skarbiec";
pub(crate) const TARGET: &str = "lukasz-macbook";
/// The live digest the incident left quarantined on this machine.
pub(crate) const DIGEST: &str = "55f2cf470e293d03c920ee1b4184e5144c98acbc7fe6315771be892b1b9791b4";
/// The agent's own sentence for that record, copied from the host.
pub(crate) const PROBE_REASON: &str =
    "active release lost readiness: http://127.0.0.1:18788/readyz did not \
                            answer within 3s; stderr \
                            /Users/lukaszbartoszcze/.stado/logs/skarbiec-0.3.10.err: skarbiec API \
                            listening on http://127.0.0.1:18788 (loopback only)";
/// A refusal that names the candidate: the vault could not be opened at all.
pub(crate) const VAULT_REASON: &str = "candidate did not become ready within 90s: \
                            http://127.0.0.1:18895/readyz answered HTTP 503 Service Unavailable; \
                            stderr skarbiec readiness monitor: stored item cannot be decrypted: \
                            spawn gpg: No such file or directory (os error 2)";
/// The stamp both live records carry, kept so the audit assertions compare
/// against the host's own value rather than a second one invented here.
pub(crate) const QUARANTINED_AT: &str = "2026-09-17T21:50:31.922394Z";
/// The rollout generation of that host's Skarbiec declaration. Any generation
/// exercises the same branch; this is the one the incident ran under.
pub(crate) const GENERATION: u64 = 7;

/// A state directory of the shape the agent keeps, inside this package's own
/// build directory rather than the operating system's shared temporary one.
pub(crate) struct StateDir {
    dir: tempfile::TempDir,
}

impl StateDir {
    pub(crate) fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
        std::fs::create_dir_all(&root).expect("target tmp");
        Self {
            dir: tempfile::TempDir::new_in(root).expect("state dir"),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    pub(crate) fn as_str(&self) -> &str {
        self.path().to_str().expect("utf-8 state dir")
    }

    /// One rollout state document carrying exactly the quarantines given, read
    /// back through the agent's own parser so the fixture cannot describe a
    /// document the agent would refuse.
    pub(crate) fn state(&self, quarantines: &[(&str, &str)]) -> HostReleaseState {
        let mut map = serde_json::Map::new();
        for (digest, reason) in quarantines {
            map.insert(
                (*digest).to_string(),
                json!({ "reason": reason, "quarantined_at": QUARANTINED_AT }),
            );
        }
        let document = json!({
            "schema_version": STATE_SCHEMA,
            "product": PRODUCT,
            "target": TARGET,
            "rollout_generation": GENERATION,
            "phase": "quarantined",
            "quarantined": Value::Object(map),
            "detail": "desired release digest is quarantined on this host",
            "updated_at": QUARANTINED_AT,
        });
        parse_state_document(
            &serde_json::to_vec(&document).expect("document"),
            PRODUCT,
            TARGET,
            "fixture",
        )
        .expect("the fixture must be a document the agent accepts")
    }

    pub(crate) fn audit_entries(&self) -> Vec<Value> {
        let path = quarantine_audit_path(self.as_str(), PRODUCT);
        match std::fs::read_to_string(path) {
            Ok(payload) => payload
                .lines()
                .map(|line| serde_json::from_str(line).expect("one JSON object per line"))
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Write one retirement of `digest` as if it had happened `age` ago.
    pub(crate) fn seed_retirement(&self, digest: &str, age: Duration) {
        let line = json!({
            "actor": AGENT_ACTOR,
            "host": TARGET,
            "product": PRODUCT,
            "digest": digest,
            "reason": "seeded",
            "cause": "readiness_probe_unanswered",
            "audited_at": (Utc::now() - age).to_rfc3339(),
            "quarantine_reason": PROBE_REASON,
            "quarantined_at": QUARANTINED_AT,
        });
        let path = quarantine_audit_path(self.as_str(), PRODUCT);
        std::fs::write(path, format!("{line}\n")).expect("seed the trail");
    }
}
