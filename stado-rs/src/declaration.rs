//! The service declaration: the one contract a user authors to add any
//! service to the fleet.
//!
//! Stado ships no list of services and no schema per service kind. A service
//! is whatever a user declares against this contract: an immutable source the
//! bytes come from, a run spec the unit is rendered from, and — beside it in
//! the directory entry — the verification descriptor that says how the
//! service is observed and the consumers that may call it. Everything a
//! deployment knows about itself (model, engine, flags, GPU) lives inside the
//! artifact and the run spec, which Stado deliberately never parses.
//!
//! The declaration travels with the directory entry it belongs to
//! (`service_directory.services.<name>.declaration`), so older builds keep it
//! verbatim in the entry's `extra` and no writer can drop it silently.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Where the runnable bytes come from. Immutability is the whole point: a
/// source that can change under its name is how a host ends up running
/// something nobody can name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclarationSource {
    /// Immutable artifact reference, resolved the same way
    /// `stado service deploy --from-artifact` resolves it.
    pub artifact: String,
    /// Digest the installed bytes must match, 64 lowercase hex characters.
    pub sha256: String,
    /// Keys this build does not model, kept verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// What the unit runs. Opaque to Stado by design: the run spec is the
/// author's, and a fleet that parsed it would grow a schema per service
/// kind — the list this contract exists to not have.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclarationRun {
    /// Absolute path of the program, when the artifact does not name one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    /// Arguments the unit starts with.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Environment the unit runs with. Secrets are named, never written:
    /// a value here is a reference the host resolves, not the secret itself.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Keys this build does not model, kept verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The deployable half of a service declaration. The other half — how the
/// service is observed and who may call it — already lives in the directory
/// entry as `verify` and `consumers`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceDeclaration {
    pub source: DeclarationSource,
    #[serde(default, skip_serializing_if = "DeclarationRun::is_empty")]
    pub run: DeclarationRun,
    /// Keys this build does not model, kept verbatim.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl DeclarationRun {
    fn is_empty(&self) -> bool {
        self.program.is_none() && self.args.is_empty() && self.env.is_empty() && self.extra.is_empty()
    }
}

impl ServiceDeclaration {
    /// The declaration inside one directory entry, when the entry carries
    /// one. Read through here and never by keying the raw object, so the
    /// field name exists exactly once.
    pub fn from_entry(entry: &Value) -> Option<Self> {
        entry
            .get("declaration")
            .and_then(|value| serde_json::from_value(value.clone()).ok())
    }
}

/// One reference the host will resolve: no control characters, no `..`, no
/// leading `/` for store references (paths under `run.program` are host
/// paths and take the path rule instead).
fn safe_reference(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && !value.contains("..")
        && !value.chars().any(char::is_control)
}

/// One host path: absolute, no `..`, no control characters — the same rule
/// the directory authority's command path answers to.
fn safe_host_path(value: &str) -> bool {
    std::path::Path::new(value).is_absolute()
        && !value.contains("..")
        && !value.chars().any(char::is_control)
}

/// Every problem with one declaration, located for its author.
///
/// Called where the registry is validated, for the same reason
/// [`crate::targets::validate_verification`] is: a declaration no build can
/// deploy is a promise nobody keeps, and the person typing it is the one who
/// can fix it. Every problem rather than the first, because a document that
/// needs a signing key to rewrite must not cost two trips.
pub fn validate(location: &str, declaration: &ServiceDeclaration) -> Vec<String> {
    let mut problems = Vec::new();
    if !safe_reference(&declaration.source.artifact) {
        problems.push(format!(
            "{location}.declaration.source.artifact: must be a non-empty reference without '..'"
        ));
    }
    let sha = declaration.source.sha256.as_str();
    if !(sha.len() == 64 && sha.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()))
    {
        problems.push(format!(
            "{location}.declaration.source.sha256: must be 64 lowercase hex characters"
        ));
    }
    if let Some(program) = declaration.run.program.as_ref() {
        if !safe_host_path(program) {
            problems.push(format!(
                "{location}.declaration.run.program: must be an absolute path without '..'"
            ));
        }
    }
    for (index, arg) in declaration.run.args.iter().enumerate() {
        if arg.chars().any(char::is_control) {
            problems.push(format!(
                "{location}.declaration.run.args[{index}]: must not contain control characters"
            ));
        }
    }
    for key in declaration.run.env.keys() {
        if key.is_empty() || key.chars().any(char::is_control) {
            problems.push(format!(
                "{location}.declaration.run.env: keys must be non-empty without control characters"
            ));
        }
    }
    problems
}
