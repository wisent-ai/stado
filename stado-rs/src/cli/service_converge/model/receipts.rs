//! What one `--apply` pass produced: the deliveries it ran, what it could not
//! deliver and what it refused.

use serde_json::{json, Value};

/// One delivery run for a `host-behind` binary.
pub(in crate::cli::service_converge) struct Released {
    pub(in crate::cli::service_converge) binary: String,
    pub(in crate::cli::service_converge) version: String,
    pub(in crate::cli::service_converge) status: &'static str,
    pub(in crate::cli::service_converge) detail: String,
}

impl Released {
    pub(in crate::cli::service_converge) fn to_json(&self) -> Value {
        json!({
            "binary": self.binary,
            "version": self.version,
            "status": self.status,
            "detail": self.detail,
        })
    }
}

/// What `--apply` found behind its declaration and could do nothing about,
/// kept apart from the
/// deliveries on purpose: a binary the release declaration cannot carry
/// produced no delivery at all, and counting it as a failed one would report
/// an attempt that never happened.
pub(in crate::cli::service_converge) struct Undeliverable {
    pub(in crate::cli::service_converge) binary: String,
    pub(in crate::cli::service_converge) detail: String,
}

impl Undeliverable {
    pub(in crate::cli::service_converge) fn to_json(&self) -> Value {
        json!({
            "binary": self.binary,
            "detail": self.detail,
        })
    }
}

/// What `--apply` refused to do: the host runs a version strictly NEWER than
/// the declaration, so delivering the declared one would be a downgrade of a
/// live host. Kept apart from both the deliveries and the undeliverable:
/// nothing was attempted, and the remedy moves the declaration, not the host.
pub(in crate::cli::service_converge) struct Refused {
    pub(in crate::cli::service_converge) binary: String,
    pub(in crate::cli::service_converge) declared: String,
    pub(in crate::cli::service_converge) installed: String,
    /// The exact command that moves the declaration to the observed version.
    pub(in crate::cli::service_converge) remediation: String,
}

impl Refused {
    pub(in crate::cli::service_converge) fn to_json(&self) -> Value {
        json!({
            "binary": self.binary,
            "declared_version": self.declared,
            "installed_version": self.installed,
            "remediation": self.remediation,
        })
    }
}

pub(in crate::cli::service_converge) const COMPLETED: &str = "completed";
pub(in crate::cli::service_converge) const FAILED: &str = "failed";

/// Everything one `--apply` pass did: the releases it ran, the `host-behind`
/// binaries it could not run one for, and the downgrades it refused.
#[derive(Default)]
pub(in crate::cli::service_converge) struct AppliedPass {
    pub(in crate::cli::service_converge) releases: Vec<Released>,
    pub(in crate::cli::service_converge) undeliverable: Vec<Undeliverable>,
    pub(in crate::cli::service_converge) refused: Vec<Refused>,
}
