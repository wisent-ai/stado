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
    if let (Some(average), Some(samples)) = (
        document
            .pointer("/outcomes/throughput/average_wall_seconds_per_completed_job")
            .and_then(Value::as_f64),
        document
            .pointer("/outcomes/throughput/samples")
            .and_then(Value::as_u64),
    ) {
        println!("completed jobs ran {average:.0} s on average over {samples} measured");
    }
    if let Some(unreadable) = document
        .pointer("/outcomes/throughput/unreadable_stamps")
        .and_then(Value::as_array)
    {
        for stamp in unreadable {
            println!(
                "job {} has a {} that does not read ({}), so it is not measured: {}",
                stamp["job_id"], stamp["field"], stamp["value"], stamp["error"]
            );
        }
    }
    if let Some(error) = document.get("measurements_error").and_then(Value::as_str) {
        println!("capability measurements could not be read: {error}");
    }
    fleet::print_fleet(document, claim);
    quota::print_quota(document);
    money::print_money(document);
}
