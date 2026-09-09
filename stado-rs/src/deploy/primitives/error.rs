//! The deploy layer's failure type.

/// Deploy-layer failure carrying the exact Python exception message
/// (RuntimeError / ValueError / LookupError text). The CLI maps it to a
/// click-`ClickException`-style `Error: {msg}` on stderr, exit 1.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DeployError(pub String);

impl From<String> for DeployError {
    fn from(msg: String) -> Self {
        Self(msg)
    }
}

impl From<&str> for DeployError {
    fn from(msg: &str) -> Self {
        Self(msg.to_string())
    }
}
