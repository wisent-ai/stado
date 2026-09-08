//! A refused conditional write, kept as one value so the operator sentence,
//! the typed receipt and the exit code cannot disagree about what happened.

use crate::cli::CmdError;

/// Exit code for a registry write refused because the document had already
/// moved: `sysexits.h`'s `EX_TEMPFAIL`, "try again".
///
/// A reconcile loop has to tell "somebody wrote first, so re-read and
/// re-apply" from "the store is broken" without reading English. Both were
/// [`super::CLICK_ERROR_CODE`](crate::cli::CLICK_ERROR_CODE), so a loop either treated a lost race as fatal
/// or retried a genuine outage forever. Storage and validation failures keep
/// exit 1; only a lost condition is 75, and [`super::main_entry`](crate::cli::main_entry) passes any
/// code other than 1 through unremapped.
pub const REGISTRY_CONFLICT_EXIT: i32 = 75;

/// What the canonical object turned out to be when a conditional write was
/// refused — the one input the operator sentence, the typed receipt and the
/// exit code are all derived from.
pub(in crate::cli::registry) enum RegistryActual {
    /// The object is there, at a generation that is not the caller's.
    Generation(String),
    /// There is no object at all, so no token can match one.
    Absent,
    /// The generation matched when it was read and had moved by the swap. The
    /// generation it carries now is whatever the winning writer produced, and
    /// this command deliberately does not go back to read it: that answer
    /// would be one more race, and the caller has to re-read anyway.
    Raced,
}

/// A refused conditional write, kept as data until the caller decides which
/// of its faces it needs.
///
/// One place produces all three — the sentence on stderr, the `conflict`
/// receipt on stdout and [`REGISTRY_CONFLICT_EXIT`] — so a machine reading
/// the receipt and an operator reading the sentence can never disagree about
/// what happened.
pub(in crate::cli::registry) struct RegistryConflict {
    pub(in crate::cli::registry) location: String,
    pub(in crate::cli::registry) expected: String,
    pub(in crate::cli::registry) actual: RegistryActual,
}

impl RegistryConflict {
    /// The generation the object carries instead, when it is a generation at
    /// all: the receipt's `actual_generation`.
    pub(in crate::cli::registry) fn actual_generation(&self) -> Option<&str> {
        match &self.actual {
            RegistryActual::Generation(version) => Some(version),
            RegistryActual::Absent | RegistryActual::Raced => None,
        }
    }

    /// The operator sentence and the machine-recognizable exit code.
    pub(in crate::cli::registry) fn error(&self) -> CmdError {
        let observed = match &self.actual {
            RegistryActual::Generation(version) => {
                format!("{} is at generation {version}", self.location)
            }
            RegistryActual::Absent => format!("there is no document at {}", self.location),
            RegistryActual::Raced => format!(
                "{} moved between this command's read and its write",
                self.location
            ),
        };
        CmdError {
            message: Some(format!(
                "registry write refused: it is conditional on generation {} and {observed}. \
                 Another writer got there first, so applying this document would erase what \
                 they published. Re-read the registry (`stado registry pull --with-generation`), \
                 re-apply the change to what it now says, and write again with the new token. \
                 Exit {REGISTRY_CONFLICT_EXIT} means exactly this and nothing else: the store \
                 is healthy and the document is valid.",
                self.expected
            )),
            code: REGISTRY_CONFLICT_EXIT,
            ..CmdError::default()
        }
    }
}

/// Why a registry write did not happen, split so [`push`](crate::cli::registry::push) can answer a lost
/// condition with a receipt and everything else with the failure it is.
pub(in crate::cli::registry) enum RegistryWriteError {
    Conflict(RegistryConflict),
    Failed(CmdError),
}

impl From<RegistryWriteError> for CmdError {
    fn from(error: RegistryWriteError) -> Self {
        match error {
            RegistryWriteError::Conflict(conflict) => conflict.error(),
            RegistryWriteError::Failed(error) => error,
        }
    }
}
