//! The az-replies error, the injectable az CLI seam, the production
//! Azure Support REST runner bound to it, and the JSON wrapper every
//! read helper goes through.

mod rest;

use serde_json::{json, Value};

use self::rest::run_azure_rest;

/// az-replies error. A non-success Azure Support answer surfaces immediately
/// (so a misconfigured Azure auth is not read as empty results that look like
/// 'nothing to do'), and each other cause keeps its own variant so its fleet
/// failure class is known rather than guessed from the sentence.
#[derive(Debug, thiserror::Error)]
pub enum RepliesError {
    /// Azure Support answered with a non-success HTTP status.
    #[error("Command '{cmd}' returned non-zero exit status {code}.")]
    CalledProcess {
        cmd: String,
        code: i32,
        stderr: String,
    },
    /// The runner is not configured for this call: no subscription, or an
    /// operation it does not implement.
    #[error("{0}")]
    Config(String),
    /// No Azure bearer token could be obtained.
    #[error("no Azure credentials for Azure Support: {0}")]
    Auth(String),
    /// Azure Support could not be reached or its answer could not be read.
    #[error(transparent)]
    Transport(#[from] reqwest::Error),
    /// Azure Support answered with something that is not JSON.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl RepliesError {
    /// Python `exc.stderr` for the CLI's Forbidden/permission check.
    pub fn stderr(&self) -> &str {
        match self {
            RepliesError::CalledProcess { stderr, .. } => stderr,
            _ => "",
        }
    }

    /// The fleet failure class: Azure's own status decides an HTTP refusal (a
    /// 4xx it leaves unclassified is still Azure refusing the request), a
    /// runner without subscription or operation is configuration, a missing
    /// token is authentication, and an unreachable or garbled Azure Support is
    /// its outage.
    pub fn failure_code(&self) -> crate::primitives::failure::FailureCode {
        use crate::primitives::failure::FailureCode;
        match self {
            Self::CalledProcess { code, .. } => match u16::try_from(*code) {
                Ok(status) => match FailureCode::from_upstream_status(status) {
                    FailureCode::Unknown if (400..500).contains(&status) => FailureCode::Refused,
                    known => known,
                },
                Err(_) => FailureCode::Unknown,
            },
            Self::Config(_) => FailureCode::Config,
            Self::Auth(_) => FailureCode::Auth,
            Self::Transport(error) if error.is_timeout() => FailureCode::Timeout,
            Self::Transport(_) | Self::Json(_) => FailureCode::InfraDown,
        }
    }
}

/// Injectable az CLI runner (Python `subprocess.run(["az", *args, "-o",
/// "json"], check=True, capture_output=True, text=True)`). Implementations
/// return raw stdout; non-zero exit maps to
/// [`RepliesError::CalledProcess`].
pub trait AzRunner {
    fn run(&self, args: &[&str]) -> Result<String, RepliesError>;
}

/// Production Azure Support REST runner. The synchronous trait is retained for
/// deterministic fixtures; production bridges into the existing Tokio runtime.
pub struct SystemAzRunner;

impl AzRunner for SystemAzRunner {
    fn run(&self, args: &[&str]) -> Result<String, RepliesError> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(run_azure_rest(args))
        })
    }
}

/// Python `_az`: invoke az returning parsed JSON ([] on empty stdout).
pub(super) fn az(runner: &dyn AzRunner, args: &[&str]) -> Result<Value, RepliesError> {
    let stdout = runner.run(args)?;
    if stdout.trim().is_empty() {
        return Ok(json!([]));
    }
    Ok(serde_json::from_str(&stdout)?)
}
