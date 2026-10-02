//! What a failed `stado` command carries out of the process.
//!
//! One type, [`CmdError`], the exit code every runtime failure defaults to,
//! and the conversions that let each layer below the CLI answer with `?`.
//! [`http_failure`] joins a reqwest cause chain into the sentence an operator
//! actually gets to read.

/// Command failure with a click-matching exit code. A `Some` message is
/// printed as `Error: {msg}` on stderr (click `ClickException`, code 1)
/// followed by the classified operator line, and the process exits with
/// [`crate::primitives::failure::FailureCode::exit_code`] applied to `code`; a `None`
/// message exits silently (click `SystemExit`, e.g. config validation
/// failure after the ERROR lines were already printed).
#[derive(Debug, Default)]
pub struct CmdError {
    pub message: Option<String>,
    pub code: i32,
    /// The failure code this error stated about itself where it was built.
    ///
    /// `None` means the code that failed stated none, and the failure is
    /// reported as `unknown`: nothing reads the wording, because a keyword
    /// read of a sentence is a guess — a hard allowlist refusal whose text
    /// happens to print an option named `--login-timeout-ms` would be read as
    /// a retryable timeout. A caller that knows what its failure is says so
    /// here.
    pub failure: Option<crate::primitives::failure::FailureCode>,
    /// Operator help that belongs beside the failure but not inside it —
    /// the approved spellings of a refused command, for instance. Printed
    /// after the error line, carried as its own field in `--json`, and
    /// never classified or logged as the failure's detail.
    pub help: Option<String>,
    /// The caller was invoked with `--json` and its failure must be
    /// machine-readable too.
    ///
    /// A command that answers `--json` with prose on the error path cannot
    /// be handled by the script that asked for JSON; it can only be parsed
    /// by eye. [`main_entry`](crate::cli::main_entry) prints one envelope for every command that
    /// sets this, so the shape is uniform rather than per-command.
    pub json: bool,
}

/// click `ClickException`'s exit code: "it ran and failed", the C library's
/// `EXIT_FAILURE`. Every runtime failure has used it since the Python
/// original, and it stays the default — only a retryable failure is
/// remapped, in [`main_entry`](crate::cli::main_entry).
pub const CLICK_ERROR_CODE: i32 = nix::libc::EXIT_FAILURE;

impl CmdError {
    /// click `ClickException`: "Error: {msg}" on stderr, exit 1.
    pub fn click(msg: impl Into<String>) -> Self {
        Self {
            message: Some(msg.into()),
            code: CLICK_ERROR_CODE,
            ..Self::default()
        }
    }

    /// A rule refused the request: printed like [`Self::click`], and stated
    /// as `refused` so the operator line names a refusal instead of an
    /// unattributed failure. Use it wherever the message states the rule.
    pub fn refused(msg: impl Into<String>) -> Self {
        Self::click(msg).stating(crate::primitives::failure::FailureCode::Refused)
    }

    /// click `UsageError`: "Error: {msg}" on stderr, exiting with the code
    /// clap's own parse failures exit with — "you invoked this wrongly", as
    /// distinct from [`Self::click`]'s "it ran and failed". A usage error is
    /// the request refused for its own form, so it is stated `refused` here,
    /// once for every caller, and never left to the wording classifier to
    /// report as an unattributed failure of Stado.
    pub fn usage(msg: impl Into<String>) -> Self {
        Self {
            message: Some(msg.into()),
            code: clap::Error::new(clap::error::ErrorKind::InvalidValue).exit_code(),
            failure: Some(crate::primitives::failure::FailureCode::Refused),
            ..Self::default()
        }
    }

    /// click `SystemExit(code)`: nothing more to print.
    pub fn silent(code: i32) -> Self {
        Self {
            message: None,
            code,
            ..Self::default()
        }
    }

    /// Carry the code the failure already knows, so nothing downstream has
    /// to infer it from the wording.
    pub fn stating(mut self, code: crate::primitives::failure::FailureCode) -> Self {
        self.failure = Some(code);
        self
    }

    /// Attach operator help that is not part of the failure sentence.
    pub fn helping(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    /// Answer in JSON, because that is what the caller asked for.
    pub fn machine_readable(mut self, json: bool) -> Self {
        self.json = json;
        self
    }
}

impl std::fmt::Display for CmdError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.message.as_deref() {
            Some(message) => formatter.write_str(message),
            None => write!(formatter, "command failed with exit code {}", self.code),
        }
    }
}

impl std::error::Error for CmdError {}

impl From<String> for CmdError {
    fn from(msg: String) -> Self {
        Self::click(msg)
    }
}

impl From<&str> for CmdError {
    fn from(msg: &str) -> Self {
        Self::click(msg)
    }
}

impl From<crate::queue::submit::SubmitError> for CmdError {
    fn from(exc: crate::queue::submit::SubmitError) -> Self {
        Self::click(exc.to_string())
    }
}

impl From<crate::queue::StorageError> for CmdError {
    fn from(exc: crate::queue::StorageError) -> Self {
        match exc {
            crate::queue::StorageError::Http(error) => Self::from(error),
            error => Self::click(error.to_string()),
        }
    }
}

impl From<crate::profiles::ProfileError> for CmdError {
    fn from(exc: crate::profiles::ProfileError) -> Self {
        Self::click(exc.to_string())
    }
}

impl From<crate::config_file::ConfigError> for CmdError {
    fn from(exc: crate::config_file::ConfigError) -> Self {
        Self::click(exc.to_string())
    }
}

impl From<serde_json::Error> for CmdError {
    fn from(exc: serde_json::Error) -> Self {
        Self::click(exc.to_string())
    }
}

impl From<std::io::Error> for CmdError {
    fn from(exc: std::io::Error) -> Self {
        Self::click(exc.to_string())
    }
}

/// The whole cause chain of one HTTP failure, joined, with the URL it was
/// asking for.
///
/// `reqwest::Error`'s own `Display` is frequently one unattributable word, and
/// `builder error` is the worst of them: it names no URL, no header and no
/// field. It can be the only thing `stado storage stat` says for one
/// product's catalog object while the same command for other products
/// answers an honest HTTP 401 — so the operator's only signal that the
/// fault is in a credential rather than in the network is that one product
/// differs from the others. The answer is
/// one layer down, in a source chain nothing printed: a header value that
/// could not be built. Every reqwest failure that reaches an operator now
/// carries that chain, because the layer that knows the cause is never the one
/// whose message gets shown.
pub fn http_failure(error: &reqwest::Error) -> String {
    let mut message = error.to_string();
    let mut cause: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(error);
    while let Some(current) = cause {
        let text = current.to_string();
        if !text.is_empty() && !message.contains(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }
        cause = current.source();
    }
    if let Some(url) = error.url() {
        message.push_str(&format!(" (requesting {url})"));
    }
    message
}

impl From<reqwest::Error> for CmdError {
    fn from(exc: reqwest::Error) -> Self {
        let failure = exc
            .is_connect()
            .then_some(crate::primitives::failure::FailureCode::InfraDown);
        Self {
            failure,
            ..Self::click(http_failure(&exc))
        }
    }
}

impl From<crate::providers::ProviderError> for CmdError {
    fn from(exc: crate::providers::ProviderError) -> Self {
        match exc {
            error @ (crate::providers::ProviderError::Disabled(_)
            | crate::providers::ProviderError::NotEnabled(_)) => Self::refused(error.to_string()),
            error => Self::click(error.to_string()),
        }
    }
}
