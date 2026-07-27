//! `stado coordinator` — port of the `coordinator` command in
//! `stado/cli.py`: runs the scheduling tick locally instead of the GCP
//! Cloud Function (see `crate::coordinator`).

use super::CmdError;

/// Python raises `SystemExit(run_coordinator(...))`: 0 is success, a
/// message is a fatal exit 1.
pub async fn run(target: Option<String>, once: bool) -> Result<(), CmdError> {
    match crate::coordinator::run(target.as_deref(), once).await {
        Ok(0) => Ok(()),
        Ok(code) => Err(CmdError::silent(code)),
        Err(message) => Err(CmdError::click(message)),
    }
}
