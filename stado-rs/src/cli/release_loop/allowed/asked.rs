//! Oko's admission asked for one named session: the work a queue runs for a
//! candidate another session submitted is that session's work, so Oko is
//! told the submitter, never the queue's own session, and nothing is
//! inherited from or passed on to the queue's process.

use super::{decision, Decision, Unanswered};

/// `action of product`, or the action alone.
pub(super) fn scope(action: &str, product: Option<&str>) -> String {
    match product {
        Some(product) => format!("{action} of {product}"),
        None => action.to_string(),
    }
}

/// Oko's admission of `action` as work `session` asked for, whichever
/// session runs this process: Oko's reason, or the refusal naming the
/// ruling that decided it.
pub async fn refusal_as(
    action: &str,
    product: Option<&str>,
    purposes: &[&str],
    session: Option<&str>,
) -> Result<String, String> {
    let scope = scope(action, product);
    let mut words = vec!["admit", action];
    if let Some(product) = product {
        words.extend(["--product", product]);
    }
    for purpose in purposes {
        words.extend(["--purpose", *purpose]);
    }
    if let Some(session) = session {
        words.extend(["--session", session]);
    }
    words.push("--json");
    match decision(&words).await {
        Decision::Allowed(reason) => Ok(reason),
        Decision::Unanswered(Unanswered::Absent(cause)) => Ok(format!(
            "no Oko is installed on this machine ({cause}), so no ruling governs {scope}"
        )),
        Decision::Refused(reason) => Err(format!(
            "{scope} is not run: the operator's rulings do not allow it now ({reason})"
        )),
        Decision::Unanswered(Unanswered::Unread(cause)) => Err(format!(
            "{scope} is not run: Oko gave no admission decision, so whether the operator's \
             rulings allow it was never read ({cause})"
        )),
    }
}
