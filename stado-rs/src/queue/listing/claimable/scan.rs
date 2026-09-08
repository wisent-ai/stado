//! The scan request: the window, the budget, the prefilter, the caller's
//! admission rule, and the three questions both passes ask of them.

use crate::models::Job;
use crate::queue::storage::is_transition_sentinel_state;

/// What a caller can actually run, and how much scanning it will pay to find
/// it.
///
/// The window used to be counted in jobs that merely FIT the caller's VRAM,
/// while the caller then refused most of them on accelerator, platform,
/// architecture, provider, assignment, exclusivity and slot state. With a
/// centrally assigned queue that is a permanent starvation, not a hiccup: if
/// the first `want` fitting blobs all name another worker, this worker gets a
/// page of jobs it must refuse, refuses every one of them, and idles on every
/// poll while its own job sits one place past the window. So the caller's own
/// full admission predicate decides what consumes a window slot, and the
/// scanning cost is bounded separately by [`JobScan::scan_budget`] — the two
/// are different quantities and conflating them is what produced both faults.
pub struct JobScan<'a> {
    /// Jobs to return. 0 means "every eligible job in the prefix".
    pub want: usize,
    /// Job documents this scan may download while looking for them. 0 means
    /// "as many as the prefix holds". A scan that exhausts its budget returns
    /// what it found; the next poll starts from the same ordered head, so
    /// nothing is permanently unreachable.
    pub scan_budget: usize,
    /// Cheap pre-download filter off the blob's stamped `gpu_mem_gb`, so a job
    /// that cannot fit is never fetched. `i64::MAX` disables it.
    pub max_gpu_mem_gb: i64,
    /// The caller's full admission rule, applied before a job takes a window
    /// slot. It sees the listed generation of the document; a caller that
    /// re-reads the job before claiming still has to re-apply it.
    pub eligible: &'a (dyn Fn(&Job) -> bool + Sync),
    /// Anchor this scan at the index head instead of the resumable cursor,
    /// and leave the cursor where it was.
    ///
    /// The cursor rotates so that a bounded poll eventually reaches work
    /// past its window — reachability. But a caller that is not asking "what
    /// can I run" and instead asking "is there anything more important than
    /// what I am running" needs the actual head of the index: a rotated
    /// window answers with the most important job in an arbitrary slice,
    /// which is not the same question. Those callers pay strict priority
    /// order for their decision and hand the rotation back untouched, so the
    /// claim loops that depend on it are unaffected.
    pub from_head: bool,
}

impl JobScan<'_> {
    pub(super) fn window_full(&self, found: usize) -> bool {
        self.want > 0 && found >= self.want
    }

    pub(super) fn budget_spent(&self, scanned: usize) -> bool {
        self.scan_budget > 0 && scanned >= self.scan_budget
    }

    pub(super) fn accepts(&self, job: &Job) -> bool {
        !is_transition_sentinel_state(&job.state)
            && job.gpu_mem_gb <= self.max_gpu_mem_gb
            && (self.eligible)(job)
    }
}
