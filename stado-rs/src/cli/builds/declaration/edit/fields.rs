//! The fields an edit may name, and how one field is replaced: the request
//! shape `stado builds edit` parses into, the per-key replacement that
//! reports what moved, and the phrases the report is written in.

use serde_json::{Map, Value};

use crate::cli::builds::declaration::checks::{
    canonical_platforms, check_artifacts, check_branch, check_command, check_interval_seconds,
    check_repo,
};
use crate::cli::CmdError;

/// The recipe fields `stado builds edit` may replace. `None` is "the
/// operator did not name this flag, leave the field alone", which is why
/// every field is optional even though a stored recipe carries all of them.
///
/// `enabled` is absent deliberately: `enable` and `disable` own it. Whether
/// a recipe builds is a decision an operator takes on purpose, never a side
/// effect of correcting a build command.
pub(in crate::cli::builds) struct RecipeEdit {
    pub(in crate::cli::builds) repo: Option<String>,
    pub(in crate::cli::builds) branch: Option<String>,
    pub(in crate::cli::builds) command: Option<String>,
    /// Given at all, the paths REPLACE the recorded list.
    pub(in crate::cli::builds) artifacts: Option<Vec<String>>,
    /// Given at all, the platforms REPLACE the recorded list.
    pub(in crate::cli::builds) platforms: Option<Vec<String>>,
    pub(in crate::cli::builds) auto_declare: Option<bool>,
    pub(in crate::cli::builds) interval_seconds: Option<u64>,
}

impl RecipeEdit {
    /// Whether the operator named no field at all. An `edit` that names none
    /// is a mistake, not a request to rewrite an entry with what it already
    /// says.
    pub(super) fn names_nothing(&self) -> bool {
        self.repo.is_none()
            && self.branch.is_none()
            && self.command.is_none()
            && self.artifacts.is_none()
            && self.platforms.is_none()
            && self.auto_declare.is_none()
            && self.interval_seconds.is_none()
    }

    /// Every named field validated as `add` validates it, with `--platform`
    /// words canonicalized. Validation runs before the registry is read, so
    /// a rejected flag never opens a fenced write.
    pub(super) fn checked(self) -> Result<Self, CmdError> {
        if let Some(repo) = self.repo.as_deref() {
            check_repo(repo)?;
        }
        if let Some(branch) = self.branch.as_deref() {
            check_branch(branch)?;
        }
        if let Some(command) = self.command.as_deref() {
            check_command(command)?;
        }
        if let Some(artifacts) = self.artifacts.as_deref() {
            check_artifacts(artifacts)?;
        }
        if let Some(interval_seconds) = self.interval_seconds {
            check_interval_seconds(interval_seconds)?;
        }
        let platforms = match self.platforms.as_deref() {
            Some(platforms) => Some(canonical_platforms(platforms)?),
            None => None,
        };
        Ok(Self { platforms, ..self })
    }
}

/// Replace `key` with `value` unless the entry already says exactly that,
/// returning the sentence naming the change. `None` is "nothing moved": an
/// operator who re-types the current value has changed nothing, and nothing
/// is what gets reported — and, for the source, what gets cleared.
pub(super) fn replace_field(
    object: &mut Map<String, Value>,
    key: &str,
    label: &str,
    value: Value,
) -> Option<String> {
    let current = object.get(key);
    if current == Some(&value) {
        return None;
    }
    let before = current.map_or_else(|| "-".to_string(), display_field);
    let after = display_field(&value);
    object.insert(key.to_string(), value);
    Some(format!("{label} {before} → {after}"))
}

/// A recipe field as one human phrase: a string bare, a list joined, anything
/// else as the JSON it is.
fn display_field(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(display_field)
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    }
}

/// The strings of a raw JSON array field, ignoring entries that are not
/// strings — a hand-written recipe is not trusted to be well typed.
pub(super) fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// A recorded-run count as words, so a sentence about one run does not read
/// "1 recorded runs".
pub(super) fn runs_phrase(count: usize) -> String {
    match count {
        0 => "no recorded run".to_string(),
        1 => "1 recorded run".to_string(),
        many => format!("{many} recorded runs"),
    }
}

/// A commit sha at the length `builds list` prints it.
pub(super) fn short_ref(sha: &str) -> String {
    sha.chars().take(8).collect()
}
