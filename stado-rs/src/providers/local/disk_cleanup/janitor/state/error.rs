//! The janitor failure type, carrying the Python exception type name
//! the report records.

use std::io;

// ---------------------------------------------------------------------------
// errors (Python exception type names, bounded — `_error_code`)
// ---------------------------------------------------------------------------

/// A janitor failure carrying the Python exception TYPE NAME the report
/// records (`_error_code(exc) = type(exc).__name__[:80]`) plus a private
/// detail message that never enters the report.
#[derive(Debug)]
pub struct JanitorError {
    pub code: &'static str,
    pub message: String,
}

impl JanitorError {
    pub fn os(message: &str) -> Self {
        Self {
            code: "OSError",
            message: message.to_string(),
        }
    }
    pub fn timeout(message: &str) -> Self {
        Self {
            code: "TimeoutError",
            message: message.to_string(),
        }
    }
    pub fn blocking(message: &str) -> Self {
        Self {
            code: "BlockingIOError",
            message: message.to_string(),
        }
    }
    pub fn lookup(message: &str) -> Self {
        Self {
            code: "LookupError",
            message: message.to_string(),
        }
    }
    pub fn value(message: &str) -> Self {
        Self {
            code: "ValueError",
            message: message.to_string(),
        }
    }
    /// This build refused a WELL-FORMED registry: the document parsed, and
    /// declares something this binary has no implementation for.
    ///
    /// Distinct from [`JanitorError::value`] because the journal entry is the
    /// operator's only signal, and `policy:ValueError` says "the registry is
    /// invalid" — which was false for all 8348 refusals between
    /// 2026-08-20 and 2026-09-02. The registry was valid; the running process
    /// was older than it. Three build eras refused today's document for three
    /// different reasons (an unknown cleaner name, then a changed required
    /// field set), each indistinguishable in the journal from a corrupt file,
    /// and each cleared only by an unrelated restart onto a newer build.
    ///
    /// `NotImplementedError` keeps the file's convention — codes are Python
    /// exception TYPE NAMES, and this is the builtin Python raises for an
    /// operation the running code does not implement. It is also strictly
    /// more specific than the `RuntimeError` it derives from, which this
    /// crate already spends on HTTP and state-machine failures elsewhere.
    ///
    /// The distinction is the CODE, not the area. The area is the janitor's
    /// coarse lifecycle stage (`runtime` before policy resolves, `policy`
    /// once it is being resolved) and a refusal happens squarely inside
    /// policy resolution; the code is this file's "what kind of failure" axis
    /// throughout. Nothing but this error crosses the boundary out of
    /// [`resolve_canonical_policy`], so a new area would have to be carried
    /// on the error anyway — the same field, spelled less accurately.
    ///
    /// The message stays private, as it does for every other constructor:
    /// [`JanitorError::error_code`] records the type name alone, and a
    /// rejection sentence names field paths.
    pub fn unsupported(message: &str) -> Self {
        Self {
            code: "NotImplementedError",
            message: message.to_string(),
        }
    }
    /// Python `_error_code`: bounded diagnostics without paths, values,
    /// or credentials.
    pub fn error_code(&self) -> String {
        self.code.chars().take(80).collect()
    }
}

impl std::fmt::Display for JanitorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for JanitorError {}

impl From<io::Error> for JanitorError {
    fn from(exc: io::Error) -> Self {
        let code = match exc.kind() {
            io::ErrorKind::NotFound => "FileNotFoundError",
            io::ErrorKind::PermissionDenied => "PermissionError",
            io::ErrorKind::TimedOut => "TimeoutError",
            io::ErrorKind::WouldBlock => "BlockingIOError",
            _ => "OSError",
        };
        Self {
            code,
            message: exc.to_string(),
        }
    }
}

impl From<serde_json::Error> for JanitorError {
    fn from(exc: serde_json::Error) -> Self {
        Self {
            code: "ValueError",
            message: exc.to_string(),
        }
    }
}

impl From<crate::queue::StorageError> for JanitorError {
    fn from(exc: crate::queue::StorageError) -> Self {
        // The Python fetches via the GCS SDK and lets its exceptions
        // propagate; the report records only the (bounded) type name.
        Self {
            code: "OSError",
            message: exc.to_string(),
        }
    }
}
