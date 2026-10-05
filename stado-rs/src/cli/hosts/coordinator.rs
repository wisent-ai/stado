//! The coordinator role of `stado serve --coordinator <entry>`: runs the
//! scheduling tick locally (see `crate::coordinator`).

use crate::cli::CmdError;

/// 0 is success, a message is a fatal exit 1.
pub async fn run(
    target: Option<String>,
    invocation: crate::coordinator::Invocation,
) -> Result<(), CmdError> {
    match crate::coordinator::run(target.as_deref(), invocation).await {
        Ok(0) => Ok(()),
        Ok(code) => Err(CmdError::silent(code)),
        Err(message) => Err(CmdError::click(message)),
    }
}
