//! The blob prefix run manifests live under, and the lifecycle prefixes a
//! member job can sit in.

/// Blob prefix holding run manifests.
pub const RUN_PREFIX: &str = "runs";
/// Prefixes a job can no longer leave.
pub const TERMINAL_PREFIXES: [&str; 4] = ["completed", "uploaded", "failed", "cancelled"];
/// Every prefix a member job can sit in (probe order).
pub const ALL_PREFIXES: [&str; 6] = [
    "queue",
    "running",
    "completed",
    "uploaded",
    "failed",
    "cancelled",
];
