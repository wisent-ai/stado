//! Oko's admission asked for one named session: the work a queue runs for a
//! candidate another session submitted is that session's work, so Oko is
//! told the submitter, never the queue's own session, and nothing is
//! inherited from or passed on to the queue's process.

use super::{admitted_above, calling_session, decision, inherited, Decision, Unanswered};

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
    decided_as(action, product, purposes, session).await?
}

/// Oko's decision on `action` as work `session` asked for: `Ok(Ok(reason))`
/// when it may run, `Ok(Err(refusal))` when the operator's rulings rule it
/// out, naming the ruling, and `Err` when no decision was read at all.
async fn decided_as(
    action: &str,
    product: Option<&str>,
    purposes: &[&str],
    session: Option<&str>,
) -> Result<Result<String, String>, String> {
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
        Decision::Allowed(reason) => Ok(Ok(reason)),
        Decision::Unanswered(Unanswered::Absent(cause)) => Ok(Ok(format!(
            "no Oko is installed on this machine ({cause}), so no ruling governs {scope}"
        ))),
        Decision::Refused(reason) => Ok(Err(format!(
            "{scope} is not run: the operator's rulings do not allow it now ({reason})"
        ))),
        Decision::Unanswered(Unanswered::Unread(cause)) => Err(format!(
            "{scope} is not run: Oko gave no admission decision, so whether the operator's \
             rulings allow it was never read ({cause})"
        )),
    }
}

/// Whether the operator's rulings rule `action` out now for `product` and
/// `purposes`: `None` when it may run (admitted as `refusal` admits it, the
/// admission passed on to the processes it starts), `Some(refusal)` naming
/// the ruling when they rule it out, so the caller leaves that step out and
/// says so, and `Err` when no decision could be read, which runs nothing.
pub async fn ruling(action: &str, product: Option<&str>, purposes: &[&str]) -> Result<Option<String>, String> {
    let variable = inherited(action);
    if admitted_above(&variable).await {
        return Ok(None);
    }
    let session = calling_session().await.map_err(|cause| {
        format!(
            "{} is not run: the agent session it runs under could not be read ({cause})",
            scope(action, product)
        )
    })?;
    match decided_as(action, product, purposes, session.as_deref()).await? {
        Ok(answer) => {
            std::env::set_var(variable, format!("{} {}", std::process::id(), answer));
            Ok(None)
        }
        Err(ruled_out) => Ok(Some(ruled_out)),
    }
}
