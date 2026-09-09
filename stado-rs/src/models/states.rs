//! Job lifecycle state constants, re-exported as `crate::models::job_state`.

/// Job lifecycle states. Serialized as plain strings; the state is also
/// redundantly encoded in the blob prefix (see `queue::storage`).
pub mod job_state {
    pub const QUEUED: &str = "queued";
    /// COMPLETED = extraction finished + handed off to the detached upload
    /// worker; NOT yet confirmed on HF. Kept named "completed" so the
    /// coordinator and dashboard stay unchanged.
    pub const COMPLETED: &str = "completed";
    /// UPLOADED = the upload worker confirmed the dir landed on HF (terminal).
    pub const UPLOADED: &str = "uploaded";
    pub const RUNNING: &str = "running";
    pub const FAILED: &str = "failed";
    pub const CANCELLED: &str = "cancelled";

    pub const ALL: [&str; 6] = [QUEUED, RUNNING, COMPLETED, UPLOADED, FAILED, CANCELLED];

    pub fn is_terminal(state: &str) -> bool {
        matches!(state, COMPLETED | UPLOADED | FAILED | CANCELLED)
    }
}
