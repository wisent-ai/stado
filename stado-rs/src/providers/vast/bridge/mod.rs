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
    /// Offer gone, work queued, and less free VRAM than the queued job
    /// declares it needs: something (most likely a Vast rental) holds the
    /// GPU; wisent-compute claims as soon as it is released.
    WaitingForRental { free_vram_gb: f64, need_gb: i64 },
    /// Busy and not listed: nothing to do.
    BusyNotListed,
}

/// The pure idle/unlist decision from `auto_list_loop`. `idle_dur_s` is how
/// long the queue has been idle, and is only read when `state` is idle.
pub fn decide_action(
    listed: bool,
    state: &BusyState,
    idle_dur_s: Option<i64>,
    idle_window_s: i64,
) -> AutoListAction {
    if let (true, Some(idle_dur_s)) = (state.idle, idle_dur_s) {
        return if idle_dur_s >= idle_window_s && !listed {
            AutoListAction::List { idle_dur_s }
        } else {
            AutoListAction::IdleCountdown { idle_dur_s }
        };
    }
    if listed {
        return AutoListAction::Unlist;
    }
    // The job at the head of the queue states the memory it needs; a card
    // with less free than that cannot take it, whoever holds the rest.
    match (state.queued_need_gb, state.free_vram_gb) {
        (Some(need_gb), Some(free_vram_gb)) if free_vram_gb < need_gb as f64 => {
            AutoListAction::WaitingForRental {
                free_vram_gb,
                need_gb,
            }
        }
        _ => AutoListAction::BusyNotListed,
    }
}

/// Python `auto_list_loop` keyword arguments as a struct.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoListParams {
    /// Wisent-compute must be idle this many consecutive seconds before
    /// listing.
    pub idle_window_s: i64,
    /// The operator's period between queue polls; required unless `once`.
    pub poll: Option<std::time::Duration>,
    /// Per-GPU-hour rental price USD when we list.
    pub price_gpu: f64,
    /// Per-GB-month disk price USD when we list; there is no built-in one.
    pub price_disk: f64,
    /// Caps the maximum length of any rental Vast can hand out from this
    /// offer (PUT /machines/create_asks/ duration field, vast-cli
    /// vast.py:
    /// 8092). With duration_s=3600 the worst-case wait for a
    /// wisent-compute job behind an active Vast rental is one hour; None
    /// leaves the offer open-ended.
    pub duration_s: Option<i64>,
    /// Print the toggle decisions without calling the Vast API.
    pub dry_run: bool,
    /// Evaluate one poll and return, instead of looping forever. The
    /// graphical surface previews the bridge this way: a daemon cannot be
    /// run through a request that has to answer.
    pub once: bool,
}
