//! Standing checks for the shape of the fleet: is what is declared what is
//! running, and does anything measure the difference.
//!
//! NO Python original. Written on 2026-08-31 after a night in which seven
//! defects of ONE shape were fixed by hand and nothing in the product would
//! have caught the eighth. Every check here is a question somebody had to ask
//! a host by hand that night, and the answer each time was a surprise:
//!
//! - three processes served one declared port on `charless-mac-mini`
//!   (`127.0.0.1:8765`, `[::1]:8765`, and a `node` on the tailnet address),
//!   found with `lsof` after hours of treating the symptom as a slow link;
//! - a label declared in two launchd domains ran twice and was invisible to
//!   `service list --undeclared`, precisely BECAUSE the label was declared;
//! - the live object API answered `healthz` 200 while every object route
//!   returned 503, so the health check was green on a server refusing its
//!   entire purpose;
//! - a primary addressed by bare key with a replica addressed by qualified
//!   path silently produced 48 GiB of objects nothing could resolve;
//! - a managed host declared two cleaners, neither of which could reach what
//!   actually filled its disk, and nothing said so.
//!
//! The rule this module exists to enforce on itself: **a check that cannot
//! fail is the disease.** So every check reports what it MEASURED, and a check
//! that could not measure its subject says that in a finding rather than
//! passing quietly. `measured` on [`Sweep`] is the count of subjects actually
//! interrogated, and a sweep that measured nothing is not a clean sweep.
//!
//! Each finding names four things, because a verdict without them is what made
//! these take hours: the SUBJECT it is about, what the fleet DECLARES, what was
//! OBSERVED, and the exact COMMAND that resolves it.
//!
//! Two entry points, one implementation: [`sweep`] is called by
//! [`crate::doctor`] for an operator asking now, and by
//! [`crate::coordinator`]'s tick so nobody has to ask. The tick is the reason
//! this is not another command nobody runs.

mod driver;
mod loaded;
mod naming;
mod resources;

use serde_json::{json, Value};

pub use driver::sweep;
pub use resources::stores::health_disagreement;

/// How many subjects one rule interrogated on one host.
///
/// The prose note beside it says the same thing in a sentence an operator
/// reads. This is the same number in a field something else can consume: a
/// count nobody can query is a count nobody can trend, gate or alert on, and
/// on 2026-09-03 answering "did the prefix rule actually look at anything"
/// meant parsing a 54,894-character string by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Measurement {
    /// Stable id of the rule that did the interrogating, or of the check
    /// itself when the count is not per-rule.
    pub check: &'static str,
    /// The host the subjects were counted on, when the count is per-host.
    pub host: Option<String>,
    /// Subjects actually interrogated. Zero means this rule proved nothing
    /// here, which is a result and not a silence.
    pub subjects: u64,
}

impl Measurement {
    pub fn new(check: &'static str, host: Option<String>, subjects: u64) -> Self {
        Self {
            check,
            host,
            subjects,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "check": self.check,
            "host": self.host,
            "subjects": self.subjects,
            "proved_nothing": self.subjects == 0,
        })
    }
}

/// One thing that is not the way the fleet says it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Stable id of the check that produced it, for grepping a tick log.
    pub check: &'static str,
    /// What the finding is about: a host, a label, a port, a store.
    pub subject: String,
    /// What the fleet declares about that subject.
    pub declared: String,
    /// What was actually observed.
    pub observed: String,
    /// The exact command that resolves it.
    pub command: String,
}

impl Finding {
    pub fn to_json(&self) -> Value {
        json!({
            "check": self.check,
            "subject": self.subject,
            "declared": self.declared,
            "observed": self.observed,
            "command": self.command,
        })
    }

    /// One line carrying all four parts. The tick log is the only place some
    /// of these will ever be read, so the line has to be the whole finding.
    pub fn line(&self) -> String {
        format!(
            "{}: {} — declared {} — observed {} — fix: {}",
            self.check, self.subject, self.declared, self.observed, self.command
        )
    }
}

/// What one sweep looked at and what it found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sweep {
    pub findings: Vec<Finding>,
    /// Subjects actually interrogated. A sweep with zero of these has proven
    /// nothing, and says so instead of reading as healthy.
    pub measured: u32,
    /// Hosts the sweep could not reach at all, by name and reason.
    pub unreachable: Vec<(String, String)>,
    /// What a check measured when it had nothing to report. Present so that a
    /// silent check and a check with nothing to check are distinguishable.
    pub notes: Vec<String>,
    /// The same per-rule counts the notes carry in prose, as fields.
    pub measurements: Vec<Measurement>,
}

impl Sweep {
    fn record(&mut self, finding: Finding) {
        self.findings.push(finding);
    }

    pub fn summary(&self) -> String {
        if self.measured == 0 {
            return format!(
                "fleet shape: NOTHING measured ({} host(s) unreachable), so this is not a clean result",
                self.unreachable.len()
            );
        }
        format!(
            "fleet shape: {} subject(s) measured, {} finding(s), {} host(s) unreachable{}",
            self.measured,
            self.findings.len(),
            self.unreachable.len(),
            if self.notes.is_empty() {
                String::new()
            } else {
                format!(" — measured clean: {}", self.notes.join(", "))
            }
        )
    }
}

pub const PORT_CHECK: &str = "one-listener-per-declared-port";
pub const DOMAIN_CHECK: &str = "one-domain-per-declared-label";
pub const HEALTH_CHECK: &str = "health-green-boundaries-down";
pub const REPLICA_CHECK: &str = "replica-cannot-resolve";
pub const DISK_CHECK: &str = "disk-headroom-against-policy";
pub const PROGRAM_CHECK: &str = "loaded-label-runs-declared-program";
pub const BINARY_CHECK: &str = "loaded-label-runs-installed-binary";
pub const ARTEFACT_CHECK: &str = "service-artefact-not-older-than-installed";
pub const PREFIX_CHECK: &str = "label-carries-its-prefix-once";
pub const ORPHAN_CHECK: &str = "loaded-job-has-a-unit-file";
pub const RESTART_CHECK: &str = "job-runs-are-work-not-a-loop";
pub const SHADOW_CHECK: &str = "path-resolves-the-delivered-binary";
pub const UNIT_ENV_CHECK: &str = "unit-declares-the-environment-its-program-reads";
/// The declared half of [`PREFIX_CHECK`]: a systemd unit name carries its
/// `.service` suffix once, or it names a unit nobody wrote.
pub const SUFFIX_CHECK: &str = "unit-name-carries-its-suffix-once";
