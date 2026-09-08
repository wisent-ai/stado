//! The human-readable overview, one component per printed section.
//!
//! Every section reads the assembled document rather than the stores, so the
//! text and `--json` cannot disagree: `summary` prints the two header lines,
//! `fleet` the hosts and what they can do, `quota` the per-provider rows and
//! `money` the billing and budget blocks. The section order below is the
//! printed order.

mod fleet;
mod money;
mod quota;
mod summary;

use serde_json::Value;

use crate::deploy::fleet_claim::FleetClaim;

pub(super) fn print_human(document: &Value, claim: &FleetClaim) {
    summary::print_header(document);
    summary::print_jobs(document);
    fleet::print_fleet(document, claim);
    quota::print_quota(document);
    money::print_money(document);
}
