//! GPU detection, eligibility, capacity helpers for the local agent loop.
//!
//! Port of `stado/providers/local/helpers/__init__.py`.
//!
//! Extracted from providers/local_agent.py in Python to keep the parent
//! file under the 300-line cap. The 0.4.100 cut adds consumer_id +
//! assigned_to enforcement to _job_eligible so the coordinator's
//! centralized matcher (_assign_jobs_to_agents in coordinator.py) can pin
//! queued jobs to specific agents instead of every agent racing to claim
//! from a global FIFO. Without this enforcement, fleet-aware LPT
//! scheduling collapses to greedy first-come-first-served and the makespan
//! grows.
//!
//! Four subjects, in the order the agent loop asks about them: [`gpu`] is the
//! accelerators this host has and who else is on them, [`host`] the processor,
//! memory and staging disk it can offer, [`claims`] the rules that decide what
//! it may take, and [`running_slot`] what a job it already took is holding.

use std::sync::LazyLock;

// `_accel_hourly_rate` is NOT re-implemented here: the Python docstring
// says it "mirrors scheduler._accel_hourly_rate so both consumers apply the
// same cost-cap rule" — the Rust port shares the single implementation in
// `scheduler::scheduler::accel_hourly_rate` (re-exported for callers that
// imported it from helpers in Python).
pub use crate::scheduler::scheduler::accel_hourly_rate;

pub mod claims;
pub mod gpu;
pub mod host;
pub mod running_slot;

pub use claims::eligibility::{eligibility_refusal, job_eligible};
pub use claims::queue_scan::no_eligible_in_queue;
pub use gpu::capacity::build_capacity_dict_per_card;
pub use gpu::inventory::{detect_gpu_type, detect_local_vram_gb, smi_gpu_cards, GpuCard};
pub use gpu::vast_renter::vast_has_renter;
pub use host::cpu::{available_cpu_cores, load_average_1m, total_cpu_cores};
pub use host::job_request::{requested_cpu_cores, requested_memory_gb};
pub use host::ram::memory_gb;
pub use running_slot::{slot_is_exclusive, slot_vram, slot_waiting_for_vram};

pub(crate) use running_slot::pid_alive;

// Read by both [`claims::eligibility`] and [`running_slot`], which is why it
// stays here rather than in either of them.
static MODEL_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"--model\s+(\S+)").expect("static regex compiles"));
