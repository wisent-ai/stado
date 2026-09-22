//! What one stage and one run of reclamation are, and the small readings that
//! build them out of the marker lines a host prints.

use serde_json::Value;

use super::super::UNAVAILABLE_SUFFIX;

/// One stage, as the host measured it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stage {
    pub stage: String,
    /// `df -Pk` available blocks either side of the stage, or `None` for a
    /// stage that never ran.
    pub free_kb_before: Option<i64>,
    pub free_kb_after: Option<i64>,
    /// How many items the stage reclaimed, or in dry-run mode would reclaim.
    pub items: usize,
    /// The paths behind that count, when the stage produced them. The janitor
    /// reports per-cleaner counts and not paths, so this is empty for that
    /// stage rather than filled with placeholders.
    pub paths: Vec<String>,
    /// Why the stage could not run, for the host's own words in the rendering.
    pub detail: Option<String>,
    /// Snapshot identifiers refused by the native ownership/type checks.
    pub refused: Vec<String>,
    /// Per-workdir proof used only when the queue authority was unavailable.
    pub local_terminality_evidence: Vec<Value>,
}

/// Everything one reclamation did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reclamation {
    pub mode: String,
    pub stages: Vec<Stage>,
    pub free_kb_before: Option<i64>,
    pub free_kb_after: Option<i64>,
    /// The janitor's canonical report, kept for the per-cleaner rendering.
    pub janitor_plan: Option<Value>,
    /// Stages that did not run, and why. A stage nobody could judge must say
    /// so; reporting it as a stage that removed nothing is the same sentence
    /// as a clean host.
    pub skipped: Vec<(String, String)>,
}

/// A marker field that has to be a `df` block count.
pub(super) fn blocks(field: &str) -> Option<i64> {
    field.trim().parse::<i64>().ok()
}

/// Take every pending item line that names `stage`, leaving the rest.
pub(super) fn drain(pending: &mut Vec<(String, String)>, stage: &str) -> Vec<String> {
    let mut mine = Vec::new();
    pending.retain(|(named, path)| {
        if named == stage {
            mine.push(path.clone());
            return false;
        }
        true
    });
    mine
}

pub(super) fn drain_evidence(pending: &mut Vec<(String, Value)>, stage: &str) -> Vec<Value> {
    let mut mine = Vec::new();
    pending.retain(|(named, evidence)| {
        if named == stage {
            mine.push(evidence.clone());
            return false;
        }
        true
    });
    mine
}

/// A stage the host could not run, named as itself.
pub(super) fn unavailable(stage: &str, detail: &str) -> Stage {
    Stage {
        stage: format!("{stage}{UNAVAILABLE_SUFFIX}"),
        free_kb_before: None,
        free_kb_after: None,
        items: 0,
        paths: Vec::new(),
        detail: Some(detail.to_string()),
        refused: Vec::new(),
        local_terminality_evidence: Vec::new(),
    }
}
