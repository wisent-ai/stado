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

    /// A declaration — the registry, a catalog, a product manifest, a config
    /// file — or the local environment is wrong, whoever wrote it: stated
    /// `config`, because fixing the declaration is what helps.
    pub fn declaration(msg: impl Into<String>) -> Self {
        Self::click(msg).stating(crate::primitives::failure::FailureCode::Config)
    }

    /// What the request names does not exist where it was looked for:
    /// stated `not_found`.
    pub fn missing(msg: impl Into<String>) -> Self {
        Self::click(msg).stating(crate::primitives::failure::FailureCode::NotFound)
    }

    /// A host, a store or the network failed, or data read back is damaged:
    /// stated `infra_down`, the class a later retry can help.
    pub fn unreachable(msg: impl Into<String>) -> Self {
        Self::click(msg).stating(crate::primitives::failure::FailureCode::InfraDown)
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

    /// The same failure with what went wrong while undoing it appended.
    ///
    /// The class stays the one the original failure stated: that is what
    /// failed and what the operator acts on; the failed undo is reported
    /// beside it instead of replacing it with an unclassified sentence.
    pub fn also(mut self, detail: impl std::fmt::Display) -> Self {
        let first = self.to_string();
        self.message = Some(format!("{first}; {detail}"));
        self
    }

    /// The same failure with what was being attempted in front of it. The
    /// class stays the one the failure stated: naming the step changes the
    /// sentence, not what failed.
    pub fn within(mut self, attempt: impl std::fmt::Display) -> Self {
        let detail = self.to_string();
        self.message = Some(format!("{attempt}: {detail}"));
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

mod conversions;
mod services;
mod system;

pub use system::{
    epoch_seconds, http_failure, http_failure_code, io_failure_code, operating_system_refusal,
};
