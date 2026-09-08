//! Sentinel job states: the fence a durable transition stamps on its source
//! and the cleaned marker it leaves behind, plus the recognizers for both.

pub(in crate::queue::storage) const TRANSITION_FENCE_PREFIX: &str = "transitioning:";
pub(in crate::queue::storage) const TRANSITION_CLEANED_PREFIX: &str = "transition-cleaned:";

pub(in crate::queue::storage) fn transition_fence_state(transition_id: &str) -> String {
    format!("{TRANSITION_FENCE_PREFIX}{transition_id}")
}
pub(in crate::queue::storage) fn transition_cleaned_state(transition_id: &str) -> String {
    format!("{TRANSITION_CLEANED_PREFIX}{transition_id}")
}
pub(in crate::queue::storage) fn cleaned_transition_id(state: &str) -> Option<&str> {
    let transition_id = state.strip_prefix(TRANSITION_CLEANED_PREFIX)?;
    (transition_id.len() == 64
        && transition_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()))
    .then_some(transition_id)
}

pub(crate) fn is_transition_sentinel_state(state: &str) -> bool {
    state.starts_with(TRANSITION_FENCE_PREFIX) || state.starts_with(TRANSITION_CLEANED_PREFIX)
}
