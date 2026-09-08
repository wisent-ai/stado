//! The release worker's contract: the five variables the release pipeline
//! sets, each refused by name, and what the release log says about them.

use std::path::PathBuf;

use crate::cli::web::builds::PLATFORM;
use crate::cli::CmdError;

/// The release worker's contract, as read from the environment once.
pub(in crate::cli::web::builds) struct Worker {
    /// The checkout the worker prepared. Every command runs inside it.
    pub(in crate::cli::web::builds) source: PathBuf,
    /// Where staged files go; the release pipeline collects them from here.
    pub(in crate::cli::web::builds) output: PathBuf,
    /// The version the pipeline is cutting, which `package.json` must agree with.
    pub(in crate::cli::web::builds) version: String,
    /// The platform key the recipe declared this step under.
    pub(in crate::cli::web::builds) platform: String,
    /// Artifacts of earlier platforms, staged for this one to consume. A web
    /// product consumes none, so this may be empty; it is reported so the
    /// release log says whether anything was handed in.
    pub(in crate::cli::web::builds) inputs: String,
}

/// One variable of the worker contract, refused by name.
///
/// The shell version of this gate read `: "${WISENT_SOURCE_DIR:?Stado must
/// provide WISENT_SOURCE_DIR}"`, and the wording is kept: the operator reading
/// a failed release log needs to know the variable is Stado's to supply, not
/// something they forgot to export.
fn required(name: &str) -> Result<String, CmdError> {
    let value = std::env::var(name).unwrap_or_default();
    let value = value.trim();
    if value.is_empty() {
        return Err(CmdError::click(format!(
            "{name} is not set: Stado must provide it to a release step, so this step is running outside the release worker"
        )));
    }
    Ok(value.to_string())
}

pub(in crate::cli::web::builds) fn worker() -> Result<Worker, CmdError> {
    let source = PathBuf::from(required("WISENT_SOURCE_DIR")?);
    let output = PathBuf::from(required("WISENT_OUTPUT_DIR")?);
    let version = required("WISENT_VERSION")?;
    let platform = required("WISENT_PLATFORM")?;
    // Read but not required: a web product declares no inputs, and refusing an
    // empty value would fail every web release for a variable nothing reads.
    let inputs = std::env::var("WISENT_INPUTS_DIR").unwrap_or_default();
    if !source.is_dir() {
        return Err(CmdError::click(format!(
            "WISENT_SOURCE_DIR names {}, which is not a directory: the release worker did not prepare a checkout there",
            source.display()
        )));
    }
    Ok(Worker {
        source,
        output,
        version,
        platform,
        inputs: inputs.trim().to_string(),
    })
}

impl Worker {
    /// Refuse a platform this recipe does not describe.
    pub(in crate::cli::web::builds) fn require_web_platform(&self) -> Result<(), CmdError> {
        if self.platform != PLATFORM {
            return Err(CmdError::click(format!(
                "WISENT_PLATFORM is `{}` but this is the `{PLATFORM}` recipe: the product's .wisent-release.json calls `stado web` under a platform that is not a web platform",
                self.platform
            )));
        }
        Ok(())
    }

    /// What the release log should say about handed-in artifacts.
    pub(in crate::cli::web::builds) fn inputs_report(&self) -> String {
        if self.inputs.is_empty() {
            "no release inputs were staged for this platform".to_string()
        } else {
            format!("release inputs at {}", self.inputs)
        }
    }
}
