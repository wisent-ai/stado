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
        match exc {
            crate::queue::submit::SubmitError::Validation(message) => {
                Self::click(message).stating(crate::primitives::failure::FailureCode::Refused)
            }
            crate::queue::submit::SubmitError::Storage(error) => Self::from(error),
            crate::queue::submit::SubmitError::Io(error) => Self::from(error),
        }
    }
}

impl From<crate::queue::StorageError> for CmdError {
    fn from(exc: crate::queue::StorageError) -> Self {
        use crate::primitives::failure::FailureCode;
        use crate::queue::StorageError;
        let code = match &exc {
            StorageError::Http(_) | StorageError::Io(_) | StorageError::Json(_) => None,
            StorageError::NotFound(_) => Some(FailureCode::NotFound),
            StorageError::Auth(_) => Some(FailureCode::Auth),
            StorageError::StorageConflict(_) | StorageError::PathEscape(_) => {
                Some(FailureCode::Refused)
            }
            StorageError::Gcs { status, .. } | StorageError::Stado { status, .. } => {
                Some(FailureCode::from_upstream_status(*status))
            }
            StorageError::Other(_) => Some(FailureCode::Unknown),
        };
        match (exc, code) {
            (StorageError::Http(error), _) => Self::from(error),
            (StorageError::Io(error), _) => Self::from(error),
            (StorageError::Json(error), _) => Self::from(error),
            (error, Some(code)) => Self::click(error.to_string()).stating(code),
            (error, None) => Self::click(error.to_string()),
        }
    }
}

impl From<crate::profiles::ProfileError> for CmdError {
    fn from(exc: crate::profiles::ProfileError) -> Self {
        match exc {
            crate::profiles::ProfileError::NotFound(message) => {
                Self::click(message).stating(crate::primitives::failure::FailureCode::NotFound)
            }
            crate::profiles::ProfileError::Invalid(message) => {
                Self::click(message).stating(crate::primitives::failure::FailureCode::Config)
            }
            crate::profiles::ProfileError::Io(error) => Self::from(error),
            crate::profiles::ProfileError::Json(error) => Self::from(error),
        }
    }
}

impl From<crate::config_file::ConfigError> for CmdError {
    fn from(exc: crate::config_file::ConfigError) -> Self {
        // A configuration that cannot be read or does not validate is the
        // operator's configuration, whatever the sentence says.
        Self::click(exc.to_string()).stating(crate::primitives::failure::FailureCode::Config)
    }
}

impl From<crate::skarbiec::SkarbiecError> for CmdError {
    fn from(exc: crate::skarbiec::SkarbiecError) -> Self {
        // The vault failure states its own class: a refused grant, an absent
        // item, an unreachable vault and a misconfigured client differ.
        let code = exc.failure_code();
        Self::click(exc.to_string()).stating(code)
    }
}

impl From<crate::targets::RegistryError> for CmdError {
    fn from(exc: crate::targets::RegistryError) -> Self {
        // A registry that does not parse, holds an invalid entry or names one
        // host twice is the fleet's configuration; a registry file that
        // cannot be read states the kernel's kind.
        let code = match &exc {
            crate::targets::RegistryError::Io(_, error) => io_failure_code(error.kind()),
            crate::targets::RegistryError::Json(_)
            | crate::targets::RegistryError::InvalidEntry(_)
            | crate::targets::RegistryError::AmbiguousIdentity { .. } => {
                crate::primitives::failure::FailureCode::Config
            }
        };
        Self::click(exc.to_string()).stating(code)
    }
}

impl From<crate::targets::RegistryFetchError> for CmdError {
    fn from(exc: crate::targets::RegistryFetchError) -> Self {
        // An unreachable store is the store's outage, a store holding no
        // document has none to find, and a document that does not parse is
        // the fleet's configuration.
        let code = match &exc {
            crate::targets::RegistryFetchError::Unreachable { .. } => {
                crate::primitives::failure::FailureCode::InfraDown
            }
            crate::targets::RegistryFetchError::Absent { .. } => {
                crate::primitives::failure::FailureCode::NotFound
            }
            crate::targets::RegistryFetchError::Invalid { .. } => {
                crate::primitives::failure::FailureCode::Config
            }
        };
        Self::click(exc.to_string()).stating(code)
    }
}

impl From<crate::targets::RegistryValidationError> for CmdError {
    fn from(exc: crate::targets::RegistryValidationError) -> Self {
        // A registry document that fails validation is refused: nothing
        // publishes or acts on it.
        Self::refused(exc.to_string())
    }
}

impl From<serde_json::Error> for CmdError {
    fn from(exc: serde_json::Error) -> Self {
        // A document that is not the JSON it must be is refused input; a
        // failure to write or read the stream underneath is the stream's.
        let code = match exc.classify() {
            serde_json::error::Category::Io => crate::primitives::failure::FailureCode::Unknown,
            serde_json::error::Category::Syntax
            | serde_json::error::Category::Data
            | serde_json::error::Category::Eof => crate::primitives::failure::FailureCode::Refused,
        };
        Self::click(exc.to_string()).stating(code)
    }
}

impl From<std::io::Error> for CmdError {
    fn from(exc: std::io::Error) -> Self {
        Self::click(exc.to_string()).stating(io_failure_code(exc.kind()))
    }
}

impl From<crate::deploy::DeployError> for CmdError {
    /// The class the deploy layer stated where it raised the failure; a
    /// failure raised without one stays unclassified rather than guessed.
    fn from(exc: crate::deploy::DeployError) -> Self {
        let mut converted = Self::click(exc.message);
        converted.failure = exc.failure;
        converted
    }
}

impl From<crate::registry_import::RegistryImportError> for CmdError {
    /// A canonical registry that is already invalid is the fleet's
    /// declaration; a store that cannot be opened, written or read back the
    /// same is the store's outage.
    fn from(exc: crate::registry_import::RegistryImportError) -> Self {
        use crate::primitives::failure::FailureCode;
        use crate::registry_import::RegistryImportError;
        let code = match &exc {
            RegistryImportError::CanonicalInvalid { .. } => FailureCode::Config,
            RegistryImportError::Storage(_) | RegistryImportError::Verification => {
                FailureCode::InfraDown
            }
        };
        Self::click(exc.to_string()).stating(code)
    }
}

impl From<crate::service_resolution::ResolveError> for CmdError {
    /// A malformed directory is Config, an undeclared service NotFound, a
    /// consumer the service does not admit Refused, and a service held by a
    /// placement move InfraDown: the one answer a later call can change.
    fn from(exc: crate::service_resolution::ResolveError) -> Self {
        use crate::primitives::failure::FailureCode;
        use crate::service_resolution::ResolveError;
        let code = match &exc {
            ResolveError::Declaration(_) => FailureCode::Config,
            ResolveError::UnknownService(_) => FailureCode::NotFound,
            ResolveError::Unauthorized { .. } => FailureCode::Refused,
            ResolveError::Moving { .. } => FailureCode::InfraDown,
        };
        Self::click(exc.to_string()).stating(code)
    }
}

impl From<crate::dashboard::DashboardError> for CmdError {
    /// The store and the socket keep the classes their own conversions
    /// state; a listener failure raised as a sentence states none.
    fn from(exc: crate::dashboard::DashboardError) -> Self {
        match exc {
            crate::dashboard::DashboardError::Storage(error) => Self::from(error),
            crate::dashboard::DashboardError::Io(error) => Self::from(error),
            crate::dashboard::DashboardError::Other(message) => Self::click(message),
        }
    }
}

impl From<crate::monitor::host_health::HostHealthError> for CmdError {
    /// A host the registry does not hold and a host with no beacon are not
    /// found; a host that is not local is refused; a beacon that is not the
    /// JSON object it must be is damaged data; the registry and the store
    /// keep the classes their own conversions state.
    fn from(exc: crate::monitor::host_health::HostHealthError) -> Self {
        use crate::monitor::host_health::HostHealthError;
        use crate::primitives::failure::FailureCode;
        let message = exc.to_string();
        let code = match exc {
            HostHealthError::RegistryFetch(error) => return Self::from(error),
            HostHealthError::Registry(error) => return Self::from(error),
            HostHealthError::Storage(error) => return Self::from(error),
            HostHealthError::UnknownTarget(_) | HostHealthError::NoBeacon { .. } => {
                FailureCode::NotFound
            }
            HostHealthError::NotLocal(_) => FailureCode::Refused,
            HostHealthError::InvalidJson { .. } | HostHealthError::NotAnObject { .. } => {
                FailureCode::InfraDown
            }
        };
        Self::click(message).stating(code)
    }
}

impl From<crate::inference::plan::PlanError> for CmdError {
    /// An unset `HOME` is the environment's, an id the command never printed
    /// is the operator's input, a file operation fails by its kind, and a
    /// plan file that does not hold its plan is damaged data.
    fn from(exc: crate::inference::plan::PlanError) -> Self {
        use crate::inference::plan::PlanError;
        use crate::primitives::failure::FailureCode;
        let code = match &exc {
            PlanError::NoHome => FailureCode::Config,
            PlanError::InvalidId(_) => return Self::usage(exc.to_string()),
            PlanError::Encode(_) => FailureCode::Unknown,
            PlanError::Read(_, error) | PlanError::Write(_, error) => io_failure_code(error.kind()),
            PlanError::Invalid(..) | PlanError::Mismatched(_) => FailureCode::InfraDown,
        };
        Self::click(exc.to_string()).stating(code)
    }
}

/// The failure class an operating-system error states by its kind, read from
/// the kind the kernel returned and never from the message.
pub fn io_failure_code(kind: std::io::ErrorKind) -> crate::primitives::failure::FailureCode {
    use crate::primitives::failure::FailureCode;
    use std::io::ErrorKind;
    match kind {
        ErrorKind::NotFound => FailureCode::NotFound,
        ErrorKind::PermissionDenied => FailureCode::Auth,
        ErrorKind::TimedOut => FailureCode::Timeout,
        ErrorKind::ConnectionRefused
        | ErrorKind::ConnectionReset
        | ErrorKind::ConnectionAborted
        | ErrorKind::NotConnected
        | ErrorKind::AddrNotAvailable
        | ErrorKind::BrokenPipe => FailureCode::InfraDown,
        ErrorKind::InvalidInput | ErrorKind::InvalidData | ErrorKind::AlreadyExists => {
            FailureCode::Refused
        }
        _ => FailureCode::Unknown,
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
