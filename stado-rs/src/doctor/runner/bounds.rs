//! Selection: which probes a scope runs. Each probe answers in the time its
//! own dependency takes; the doctor sets no clock of its own over it.

use std::future::Future;

use crate::doctor::{Check, RunScope};

/// Run a selected probe. An unselected future is dropped without being
/// polled, so scoped doctor modes do not load unrelated dependencies.
pub(super) async fn selected(
    scope: RunScope,
    id: &'static str,
    title: &'static str,
    remedy: &str,
    probe: impl Future<Output = Check>,
) -> Check {
    if !scope.includes(id) {
        return Check::pass(id, title, String::new(), remedy);
    }
    probe.await
}
