//! The deploy layer's failure type.

use crate::primitives::failure::FailureCode;

/// Deploy-layer failure: the operator sentence, and the class the code that
/// raised it knew it to be.
///
/// `failure` is `None` where the raising code did not state a class; it is
/// never filled in by reading `message`. A command that turns this into a
/// `CmdError` through `From` keeps the class, so a refusal the deploy layer
/// understood is not printed as an unattributed failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct DeployError {
    pub message: String,
    pub failure: Option<FailureCode>,
}

/// A deploy failure whose class the raising code did not state.
///
/// Named like the type so the sentence-only constructor every deploy module
/// already calls (`DeployError(format!(..))`, `.map_err(DeployError)`) keeps
/// its shape; a raising site that knows its class adds `.stating(code)`.
#[allow(non_snake_case)]
pub fn DeployError(message: String) -> DeployError {
    DeployError {
        message,
        failure: None,
    }
}

impl DeployError {
    /// The class this failure is, stated where it was raised.
    pub fn stating(mut self, code: FailureCode) -> Self {
        self.failure = Some(code);
        self
    }

    /// The host channel could not carry the command: the key, the route or
    /// the process that runs it failed before the far side answered.
    pub fn unreachable(message: String) -> Self {
        DeployError(message).stating(FailureCode::InfraDown)
    }
}

impl From<String> for DeployError {
    fn from(message: String) -> Self {
        DeployError(message)
    }
}

impl From<&str> for DeployError {
    fn from(message: &str) -> Self {
        DeployError(message.to_string())
    }
}
