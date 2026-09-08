//! The words this report is read in: one host's reason, one host's answer,
//! the queued job that sizes the stall, and the rendering of its wait.

/// One reason one host cannot claim, in [`host_gates`]'s vocabulary, plus the
/// detail that sizes it.
///
/// The word and the detail are separate so the word stays greppable: an
/// operator who reads `no_capacity_publication` here has to be able to find
/// it in `stado host gates --json` and in the agent that would have published
/// it, and a word with an age or a unit label glued onto it is findable in
/// neither.
///
/// [`host_gates`]: crate::deploy::host_gates
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocker {
    /// A `host_gates` constant, verbatim.
    pub word: String,
    /// What makes it true here, in the operator's words. Empty when the word
    /// says everything.
    pub detail: String,
}

impl Blocker {
    pub(super) fn new(word: &str, detail: impl Into<String>) -> Self {
        Self {
            word: word.to_string(),
            detail: detail.into(),
        }
    }

    pub(super) fn bare(word: &str) -> Self {
        Self::new(word, "")
    }

    /// `word` or `word (detail)`.
    pub(super) fn rendered(&self) -> String {
        if self.detail.is_empty() {
            self.word.clone()
        } else {
            format!("{} ({})", self.word, self.detail)
        }
    }
}

/// One declared host's answer to "could you claim something from this queue".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostVerdict {
    /// The registry target name.
    pub host: String,
    /// `blockers.is_empty()`. A host running at its current resource limit and
    /// a host with spare resources both claim: active work is a moving queue,
    /// and calling it blocked would make this report cry wolf on every loaded
    /// box (the same rule [`host_gates::HostGates::claiming`] follows).
    ///
    /// [`host_gates::HostGates::claiming`]: crate::deploy::host_gates::HostGates::claiming
    pub claiming: bool,
    pub blockers: Vec<Blocker>,
}

/// The queued job that has waited longest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OldestWait {
    pub job_id: String,
    /// `None` for a job whose `created_at` cannot be parsed; the job is still
    /// reported, without an age, rather than dropped from the count.
    pub age_seconds: Option<i64>,
}

/// A queue wait as `121h 38m`.
///
/// Hours are never rolled into days, which is why this is not
/// [`crate::monitor::billing::humanize`]: `5d 1h 38m` reads as a backlog
/// being worked through, and the number that tells an operator this queue has
/// not moved at all is the hour count.
pub fn wait_words(seconds: i64) -> String {
    let total = u64::try_from(seconds).unwrap_or_default();
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    if hours == 0 {
        return format!("{minutes}m");
    }
    format!("{hours}h {minutes}m")
}
