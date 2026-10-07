//! What a request to the host process can fail with, stated where it fails so
//! the CLI reports a class instead of a sentence.

/// A failed request to the host process over its control socket.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ControlClientError {
    /// The socket location or the requested state file and bind are not
    /// usable as given.
    #[error("{0}")]
    Config(String),
    /// An operating-system operation on the control socket failed; its kind
    /// is the class.
    #[error("{context}: {error}")]
    Io {
        context: String,
        error: std::io::Error,
    },
    /// The host process is not running, or stopped answering mid-operation.
    #[error("{0}")]
    Unavailable(String),
    /// The socket, its peer or the owner's answer is not one this client
    /// accepts: an unrelated socket, another executable, a protocol mismatch,
    /// or the owner refusing the operation.
    #[error("{0}")]
    Refused(String),
}

impl ControlClientError {
    pub(super) fn io(context: String) -> impl FnOnce(std::io::Error) -> Self {
        move |error| Self::Io { context, error }
    }
}

/// The release agent still reports in sentences; a control failure joins
/// them with its own words.
impl From<ControlClientError> for String {
    fn from(error: ControlClientError) -> Self {
        error.to_string()
    }
}
