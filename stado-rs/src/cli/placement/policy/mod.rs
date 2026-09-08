//! `stado route placement publish` — the registry as the only writer of a
//! host's Weles placement policy, and the delta it reports back.
//!
//! The names both sides of that publication agree on live here: the basename
//! the document takes on the host, the path the worker reads it from, the
//! marker lines the apply step prints, and the two writers that stamp it.

mod document;
mod install;
mod jq;
mod receipt;

pub(crate) use document::{normalize_hostname, policy_document, policy_effect};
pub(crate) use receipt::publish_placement_policy_report;

// ---------------------------------------------------------------------------
// route placement publish
// ---------------------------------------------------------------------------

/// Basename the policy takes in the target's delivered-files directory, and the
/// only name [`apply_policy`](install::apply_policy) will read.
const POLICY_FILE: &str = "placement-policy.json";

/// Where the worker reads it, per `weles/src/worker/placement-policy.ts`: the
/// loader joins `homedir()` with `.config/weles/placement-policy.json` unless
/// `WELES_PLACEMENT_POLICY_FILE` overrides it. Reported here, never written
/// here — the move belongs to the host, because only the host can see what the
/// document replaced.
const POLICY_DESTINATION: &str = "$HOME/.config/weles/placement-policy.json";

/// `PLACEMENT_POLICY <phase> <generation> <enabled> <actions>`, tab separated:
/// the apply script's report of what the host carried and what it carries now.
const POLICY_MARKER: &str = "PLACEMENT_POLICY";

/// `PLACEMENT_VANTAGE <hostname>`: the name the host gave for itself, which is
/// the string the worker's loader will match its entry against. Printed because
/// a policy that names every host except the one it is installed on is not an
/// error the worker reports — it is a worker that declines everything.
const VANTAGE_MARKER: &str = "PLACEMENT_VANTAGE";

/// What `_source.by` names, so a file on a host traces back to the writer that
/// wrote it rather than to a machine that happened to have write access.
///
/// Two writers put this document on a host and both name themselves here: the
/// operator command below, from the coordinator through the audited channel,
/// and the host's own agent, which reconciles the same declaration on its own
/// disk. Which one wrote the file is the first question asked of a host whose
/// worker is declining rows, so the answer is in the file.
const PUBLISHED_BY: &str = "stado route placement publish";

/// [`PUBLISHED_BY`] for the host-side reconciler.
pub(crate) const RECONCILED_BY: &str = "stado agent reconcile-placement-policy";

/// The one document shape the worker's loader parses (`schema_version must be
/// 1`, `placement-policy.ts`). Publishing anything else delivers a file the
/// consumer refuses.
const POLICY_SCHEMA_VERSION: u64 = 1;
