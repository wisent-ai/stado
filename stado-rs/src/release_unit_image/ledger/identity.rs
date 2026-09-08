//! The pair of identifiers the ledger stores a file by, and the closed
//! vocabulary of what one attempt achieved.

use serde::{Deserialize, Serialize};

use crate::cli::service_refresh_image::RefreshOutcome;
use crate::deploy::service::ImageIdentity;

/// One executable file as the ledger stores it: the pair the kernel answers
/// with. Sizes, paths and link counts are deliberately absent — they move
/// without the file moving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileIdentity {
    pub device: u64,
    pub inode: u64,
}

impl FileIdentity {
    pub(in crate::release_unit_image) fn of(image: &ImageIdentity) -> Self {
        Self {
            device: image.device,
            inode: image.inode,
        }
    }

    pub(super) fn is(self, image: &ImageIdentity) -> bool {
        self.device == image.device && self.inode == image.inode
    }
}

/// What one attempt achieved.
///
/// #344's [`RefreshOutcome`] describes what a SECOND READ found, and all five
/// of its results presuppose that read. Two states of this pass have no such
/// read and must not borrow a word that claims one:
///
/// - a `kickstart` launchd refused, where recording `NotRunning` would assert
///   that nothing executes the unit's argument vector — never observed, and
///   usually false because the old process is still running the old image;
/// - an intent committed before the restart was issued whose result was never
///   written, which is what a crash anywhere in that window leaves behind. It
///   says a restart may or may not have been invoked, and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttemptOutcome {
    /// Committed BEFORE the restart is issued, and replaced by the real result
    /// on the same tick. Still present means the pass stopped between
    /// committing the intent and writing a result, so whether
    /// `launchctl kickstart` ran at all is unknown — which is exactly what
    /// this word must be read as, and no more.
    Attempting,
    /// `launchctl kickstart` did not succeed. Nothing was read afterwards and
    /// nothing is claimed about the process.
    RestartRefused,
    /// The restart was issued and the identity was read again.
    Observed(RefreshOutcome),
}

impl AttemptOutcome {
    /// The one word a report and the ledger name this by.
    pub(crate) fn word(self) -> &'static str {
        match self {
            Self::Attempting => "Attempting",
            Self::RestartRefused => "RestartRefused",
            Self::Observed(outcome) => outcome.word(),
        }
    }

    /// Decode one ledger word through the same enum values [`Self::word`]
    /// encodes.
    ///
    /// The strings live only in `word`; every candidate here is a typed
    /// outcome rather than a second list of accepted spellings.
    pub(super) fn parse(word: &str) -> Option<Self> {
        [
            Self::Attempting,
            Self::RestartRefused,
            Self::Observed(RefreshOutcome::OnDeclaredFile),
            Self::Observed(RefreshOutcome::NotRunning),
            Self::Observed(RefreshOutcome::Unread),
            Self::Observed(RefreshOutcome::Unchanged),
            Self::Observed(RefreshOutcome::StillWrong),
        ]
        .into_iter()
        .find(|outcome| outcome.word() == word)
    }
}
