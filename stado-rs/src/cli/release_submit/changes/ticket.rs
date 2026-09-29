//! Whether a stored pending change and a new handoff are the same change.

use super::Change;

/// Each field the two disagree on, with its stored and asked value, or
/// `None` when they are the same handoff. The id binds repository, product,
/// task and commit; the session is not part of it, so a ticket another
/// session wrote for the same work is this same handoff.
pub(super) fn disagreement(saved: &Change, asked: &Change) -> Option<String> {
    let differing: Vec<String> = [
        ("id", &saved.id, &asked.id),
        ("repository", &saved.repository, &asked.repository),
        ("source_commit", &saved.source_commit, &asked.source_commit),
        ("task_id", &saved.task_id, &asked.task_id),
        ("product", &saved.product, &asked.product),
    ]
    .into_iter()
    .filter(|(_, stored, wanted)| stored != wanted)
    .map(|(field, stored, wanted)| format!("{field} stored {stored}, asked {wanted}"))
    .collect();
    (!differing.is_empty()).then(|| differing.join("; "))
}
