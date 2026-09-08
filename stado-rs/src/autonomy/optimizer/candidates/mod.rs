//! Every offer turned into a priced, constraint-checked candidate.
//!
//! [`candidates_for_job`] estimates the run time, decides whether the job may
//! take preemptible capacity, finds the dearest quote among the regions the
//! offer could land in, and hands each offer to [`scoring`]. [`feedback`]
//! supplies what the recorded placements say about a target and [`shapes`]
//! what a provider shape holds, which is what the scoring rejects an offer
//! against.

mod feedback;
mod scoring;
mod shapes;

use std::collections::BTreeMap;

use crate::autonomy::cost::PriceBook;
use crate::models::Job;

// `super::storage` for the moved signatures that name
// `super::storage::<item>` verbatim in `feedback`.
use crate::autonomy::storage;

use super::offers::offer_regions;
use super::types::{CandidateContext, CapacityOffer, PlacementCandidate};

use scoring::candidate;

pub(super) fn candidates_for_job(
    job: &Job,
    offers: &[CapacityOffer],
    prices: &PriceBook,
    wall_times: &BTreeMap<(String, String), f64>,
    context: &CandidateContext<'_>,
) -> Vec<PlacementCandidate> {
    let policy = context.policy;
    offers
        .iter()
        .map(|offer| {
            let runtime = if job.runtime_seconds_estimate > f64::default() {
                job.runtime_seconds_estimate
            } else {
                crate::scheduler::cost::estimate_wall_time(
                    &job.command,
                    &offer.accelerator_type,
                    job.gpu_mem_gb,
                    wall_times,
                )
            };
            let preemptible = job.preemptible
                && policy.placement.allow_spot
                && (!policy.placement.require_checkpoint_for_spot
                    || job.max_preempts_before_ondemand > i64::default());
            let possible_regions = offer_regions(offer);
            let quote = possible_regions
                .iter()
                .filter_map(|region| {
                    prices.find_hourly(
                        offer.provider,
                        Some(region),
                        &offer.machine_type,
                        &offer.accelerator_type,
                        preemptible,
                    )
                })
                .max_by(|left, right| {
                    left.hourly_usd
                        .partial_cmp(&right.hourly_usd)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
                .or_else(|| {
                    prices.find_hourly(
                        offer.provider,
                        offer.region.as_deref(),
                        &offer.machine_type,
                        &offer.accelerator_type,
                        preemptible,
                    )
                });
            candidate(
                job,
                offer,
                runtime,
                preemptible,
                quote.as_ref(),
                &possible_regions,
                context,
            )
        })
        .collect()
}
