//! Agent-mode VM dispatch.
//!
//! Port of `stado/scheduler/dispatch/agent.py`.
//!
//! For each (accel, machine_type) bucket of queued work that isn't already
//! yielded to a local consumer, launch enough agent VMs to fill remaining
//! quota — but no more than the bucket's job count. Each VM runs
//! `wc agent --idle-shutdown`, polls the queue, packs jobs by nvidia-smi
//! VRAM, and self-terminates when no eligible queued job remains.
//!
//! Replaces the legacy 1-VM-per-job dispatch path. VRAM (read live from
//! the hardware) is the only admission constant; there is no per-VM slot
//! count.

mod launch;
mod startup;

/// Named by `crate::scheduler::scheduler`, which fills the inputs and calls
/// the dispatcher once per tick.
pub use launch::{dispatch_agent_vms, dispatch_agent_vms_with_template, AgentDispatchInputs};
/// Named by `crate::doctor::fleet::units::template`, whose preflight renders
/// the identical template the dispatcher ships.
pub(crate) use startup::bundled_template_for;
/// `deployment_substitutions` and `render_agent_startup_script` are named by
/// that same preflight; `render_startup_script` keeps the substitution pass
/// nameable at its published `dispatch::agent::` path.
pub use startup::{deployment_substitutions, render_agent_startup_script, render_startup_script};
