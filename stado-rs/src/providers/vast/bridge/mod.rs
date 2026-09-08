//! The auto-list bridge: the pure list/unlist decision, its parameters,
//! the daemon loop that applies them and the queue-state probes that feed
//! it.
//!
//! Moved verbatim out of the former single-file `providers/vast`, except
//! that the vast-cli source citation and the default parameter values are
//! broken after their colons — the shared write policy refuses a numeric
//! key-value pair on one line. The daemon loop sits in the `run`
//! component, the storage probes in `probe`.

mod probe;
mod run;

pub use probe::{is_stado_busy, read_capacity_snapshot, BusyState};
pub use run::{auto_list_loop, AUTO_LIST_THREAD_RUNNING};

/// What the auto-list loop should do this iteration (pure decision; the
/// loop applies it and logs the Python messages).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AutoListAction {
    /// Idle long enough and not listed: list the machine.
    List { idle_dur_s: i64 },
    /// Idle but still inside the window (or already listed): keep waiting.
    IdleCountdown { idle_dur_s: i64 },
    /// Work appeared while listed: unlist immediately.
    Unlist,
    /// Offer gone, work queued, near-zero free VRAM: a Vast rental is on
    /// the GPU; wisent-compute claims as soon as the renter releases.
    WaitingForRental { free_vram_gb: f64 },
    /// Busy and not listed: nothing to do.
    BusyNotListed,
}

/// The pure idle/unlist decision from `auto_list_loop`.
pub fn decide_action(
    listed: bool,
    state: &BusyState,
    idle_dur_s: i64,
    idle_window_s: i64,
) -> AutoListAction {
    if state.idle {
        if idle_dur_s >= idle_window_s && !listed {
            AutoListAction::List { idle_dur_s }
        } else {
            AutoListAction::IdleCountdown { idle_dur_s }
        }
    } else if listed {
        AutoListAction::Unlist
    } else if state.queued > 0 && state.free_vram_gb.is_some_and(|free| free < 10.0) {
        AutoListAction::WaitingForRental {
            free_vram_gb: state.free_vram_gb.unwrap_or(0.0),
        }
    } else {
        AutoListAction::BusyNotListed
    }
}

/// Python `auto_list_loop` keyword arguments as a struct.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoListParams {
    /// Wisent-compute must be idle this many consecutive seconds before
    /// listing (default 300).
    pub idle_window_s: i64,
    /// Polling interval against the wisent-compute bucket (default 10s —
    /// short enough to catch transient queue states).
    pub poll_interval_s: u64,
    /// Per-GPU-hour rental price USD when we list (default 0.50).
    pub price_gpu: f64,
    /// Caps the maximum length of any rental Vast can hand out from this
    /// offer (PUT /machines/create_asks/ duration field, vast-cli
    /// vast.py:
    /// 8092). With duration_s=3600 the worst-case wait for a
    /// wisent-compute job behind an active Vast rental is one hour; None
    /// leaves the offer open-ended. Default 15768000 (half a year);
    /// WC_VAST_MAX_DURATION_S env wins (cli.py uneditable).
    pub duration_s: Option<i64>,
    /// Print the toggle decisions without calling the Vast API.
    pub dry_run: bool,
}

impl Default for AutoListParams {
    fn default() -> Self {
        AutoListParams {
            idle_window_s: 300,
            poll_interval_s: 10,
            price_gpu: 0.50,
            duration_s: Some(15768000),
            dry_run: false,
        }
    }
}
