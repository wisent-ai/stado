//! Why the release agent gave up on a release, as the sentence it composed
//! and the cause it observed while composing it.
//!
//! The cause used to be guessed afterwards from the sentence's words. It is
//! known where the agent observes it — the pid it watches is gone, another
//! process listens on the port, the probe timed out, the manifest declares no
//! rollback compatibility — so it is carried from there as structure.

use std::fmt;

use super::QuarantineCause;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub cause: QuarantineCause,
    pub sentence: String,
}

impl Refusal {
    pub fn observed(cause: QuarantineCause, sentence: String) -> Self {
        Self { cause, sentence }
    }

    /// The same cause with the sentence wrapped, as a caller adds context.
    pub fn context(self, wrap: impl FnOnce(&str) -> String) -> Self {
        Self {
            cause: self.cause,
            sentence: wrap(&self.sentence),
        }
    }
}

/// A refusal whose cause the agent did not observe: the sentence alone.
impl From<String> for Refusal {
    fn from(sentence: String) -> Self {
        Self::observed(QuarantineCause::Unclassified, sentence)
    }
}

impl From<Refusal> for String {
    fn from(refusal: Refusal) -> Self {
        refusal.sentence
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.sentence)
    }
}
