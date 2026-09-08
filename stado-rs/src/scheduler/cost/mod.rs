//! Cost reporting + projection. Backs `stado cost report` and
//! `stado cost estimate`.
//!
//! Reads every job from JobStorage (completed/failed), computes wall-time
//! from started_at -> (completed_at or failed_at), looks up the matching
//! $/hour from catalog GPU_HOURLY_RATE_USD with SPOT_DISCOUNT applied when
//! preemptible=True, attributes each job by instance_ref (local@host vs
//! gcp:zone:instance), and aggregates per (gpu_type, target_kind, model_id).
//!
//! Replaces hand-waved cost ceilings with measured per-job distributions as
//! soon as any jobs have actually run.
//!
//! Port of `stado/scheduler/cost.py`. The wall-time medians
//! ([`wall_time_table`], [`estimate_wall_time`], [`heuristic_wall_time_seconds`])
//! are exposed for the local-pack knapsack used by the scheduler.
//!
//! The seams the module already had are the files here: `measure` holds the
//! per-job primitives (wall-clock span, catalog rate, provider/model
//! attribution), `rows` holds the row shape, the two collectors and the
//! wall-time medians, `summary` aggregates rows and renders the report
//! lines, and `projection` projects a batch from observed per-job cost.

mod measure;
mod projection;
mod rows;
mod summary;

pub use measure::attribution::{model_from_command, target_kind};
pub use measure::rates::hourly_rate_usd;
pub use projection::{project_batch, Projection};
pub use rows::collect::{collect_completed, collect_completed_dynamic};
pub use rows::row::CostRow;
pub use rows::wall_time::{estimate_wall_time, heuristic_wall_time_seconds, wall_time_table};
pub use summary::{format_report, report, BucketSummary, Report};
