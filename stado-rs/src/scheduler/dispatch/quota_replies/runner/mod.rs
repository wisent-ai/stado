//! The az-replies error, the injectable az CLI seam, the production
//! Azure Support REST runner bound to it, and the JSON wrapper every
//! read helper goes through.

mod rest;

use serde_json::{json, Value};

use self::rest::run_azure_rest;

/// az-replies error. Python raises `subprocess.CalledProcessError` on
/// non-zero exit (so a misconfigured Azure auth surfaces immediately
/// instead of producing empty results that look like 'nothing to do'),
/// `json.JSONDecodeError` on unparseable stdout, and OSError subclasses
/// when az itself cannot be spawned.
#[derive(Debug, thiserror::Error)]
pub enum RepliesError {
    /// Python `subprocess.CalledProcessError` (message matches its str()).
    #[error("Command '{cmd}' returned non-zero exit status {code}.")]
    CalledProcess {
        cmd: String,
        code: i32,
        stderr: String,
    },
    /// Python `FileNotFoundError` / `OSError` spawning az.
    #[error("{0}")]
    Spawn(String),
    /// Python `json.JSONDecodeError`.
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
