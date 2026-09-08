//! How a host is asked which version of a product it already carries.

use serde::Deserialize;

/// How the version installed on the host is read back.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Readback {
    /// Run the installed program and read its answer. `shape` is `plain`
    /// (one line, version as the last word) or `json` (an object with a
    /// `version` member): both are real, and `host inventory` had to learn
    /// the distinction after reporting `{` as skarbiec's version.
    Program { argument: String, shape: Shape },
    /// Read one top-level member of one JSON file inside the install root —
    /// `package.json` `/version` for the Weles worker, the same field the
    /// release that produced the artefact was numbered from
    /// (`weles/.wisent-release.json` `version_source`).
    JsonFile { path: String, pointer: String },
}

impl Readback {
    /// The JSON member name a `/member` pointer addresses.
    pub fn member(&self) -> Option<&str> {
        match self {
            Self::Program { .. } => None,
            Self::JsonFile { pointer, .. } => Some(pointer.trim_start_matches('/')),
        }
    }
}

/// The shape a program answers a version question in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    Plain,
    Json,
}

impl Shape {
    /// The word the remote program is bound to.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Json => "json",
        }
    }
}
