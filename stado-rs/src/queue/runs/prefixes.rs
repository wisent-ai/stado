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

/// One lifecycle prefix as a type, for code that decides by prefix: an
/// exhaustive match over it places every prefix, where a match over words
/// lets an unplaced one fall to a catch-all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prefix {
    Queue,
    Running,
    Completed,
    Uploaded,
    Failed,
    Cancelled,
}

impl Prefix {
    /// The prefix's name, as spelled above.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queue => QUEUE,
            Self::Running => RUNNING,
            Self::Completed => COMPLETED,
            Self::Uploaded => UPLOADED,
            Self::Failed => FAILED,
            Self::Cancelled => CANCELLED,
        }
    }

    /// The prefix a name spells, or `None` for a name that is not one.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            QUEUE => Some(Self::Queue),
            RUNNING => Some(Self::Running),
            COMPLETED => Some(Self::Completed),
            UPLOADED => Some(Self::Uploaded),
            FAILED => Some(Self::Failed),
            CANCELLED => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// A job under this prefix can no longer leave it.
    pub fn terminal(self) -> bool {
        match self {
            Self::Completed | Self::Uploaded | Self::Failed | Self::Cancelled => true,
            Self::Queue | Self::Running => false,
        }
    }
}
