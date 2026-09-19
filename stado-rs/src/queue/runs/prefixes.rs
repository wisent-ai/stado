//! The blob prefix run manifests live under, and the lifecycle prefixes a
//! member job can sit in.

/// Blob prefix holding run manifests.
pub const RUN_PREFIX: &str = "runs";

/// One lifecycle prefix each, named once for the whole product.
///
/// Six call sites wrote these six strings out again, each in its own order
/// — display order in `stado status`, probe order in the machine facade,
/// admission order in the queue — and the orders are deliberate, so the
/// repair is not one shared list but one shared spelling. A site declares
/// the order it needs out of these names; nobody re-types the words.
/// Asked for on 2026-09-19: "znajdz wszystkie miejsca gdzie jest obecnie
/// uzywana keywordowa logika. w jaki sposob powinno to byc naprawione".
///
/// `QUEUE` is the prefix, not the state: a job under it reports `queued`.
pub const QUEUE: &str = "queue";
pub const RUNNING: &str = "running";
pub const COMPLETED: &str = "completed";
pub const UPLOADED: &str = "uploaded";
pub const FAILED: &str = "failed";
pub const CANCELLED: &str = "cancelled";

/// Prefixes a job can no longer leave.
pub const TERMINAL_PREFIXES: [&str; 4] = [COMPLETED, UPLOADED, FAILED, CANCELLED];
/// Every prefix a member job can sit in (probe order).
pub const ALL_PREFIXES: [&str; 6] = [QUEUE, RUNNING, COMPLETED, UPLOADED, FAILED, CANCELLED];
