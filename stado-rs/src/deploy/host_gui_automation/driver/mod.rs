use super::*;

mod autologin;
mod install;
mod runtime;

pub(in crate::deploy::host_gui_automation) use autologin::reconcile_autologin;
pub(in crate::deploy::host_gui_automation) use install::reconcile_app;
pub(in crate::deploy::host_gui_automation) use runtime::reconcile_runtime;

/// Whether one user's session can be driven, and when it cannot, the item
/// that said so. A probe that only answered yes or no left the Developer ID
/// relay saying "not drivable" about a session that, asked directly, was;
/// the item is what makes the two answers comparable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionReadiness {
    pub ready: bool,
    pub reason: Option<String>,
}

async fn session_readiness_for(
    target: &ComputeTarget,
    expected_user: &str,
    readiness_key: &str,
    password: Option<&str>,
    runner: &Runner,
) -> Result<SessionReadiness, DeployError> {
    let report = status(target, password, runner).await;
    if let Some(error) = report.error {
        return Err(DeployError::unreachable(error));
    }
    Ok(readiness_from_items(
        &report.items,
        expected_user,
        readiness_key,
    ))
}

/// The companion item that says WHY a readiness key is `no`. The status pass
/// records it beside the verdict and the verdict used to drop it: the
/// Developer ID relay refused for a day with `apple-challenge-ready is no`
/// while the probe's own sentence, one item away, named what stopped it.
fn readiness_error_key(readiness_key: &str) -> &'static str {
    match readiness_key {
        "apple-challenge-ready" => "apple-challenge-preflight-error",
        _ => "accessibility-error",
    }
}

/// One session's verdict, read from a status report's items.
fn readiness_from_items(
    items: &[(String, String)],
    expected_user: &str,
    readiness_key: &str,
) -> SessionReadiness {
    let value = |key: &str| {
        items
            .iter()
            .find_map(|(name, value)| (name == key).then_some(value.as_str()))
    };
    let mut mismatches: Vec<String> = [
        ("console", expected_user),
        ("accessibility-user", expected_user),
        (readiness_key, "yes"),
    ]
    .into_iter()
    .filter(|(key, expected)| value(key) != Some(expected))
    .map(|(key, expected)| {
        format!(
            "{key} is {}, expected {expected}",
            value(key).unwrap_or("unreported")
        )
    })
    .collect();
    if !mismatches.is_empty() {
        if let Some(said) = value(readiness_error_key(readiness_key)) {
            mismatches.push(format!("{}: {said}", readiness_error_key(readiness_key)));
        }
    }
    SessionReadiness {
        ready: mismatches.is_empty(),
        reason: (!mismatches.is_empty()).then(|| mismatches.join("; ")),
    }
}

/// Whether CuaDriver can drive this exact user's current GUI session.
pub async fn automated_session_ready_for(
    target: &ComputeTarget,
    expected_user: &str,
    password: Option<&str>,
    runner: &Runner,
) -> Result<bool, DeployError> {
    automated_session_readiness_for(target, expected_user, password, runner)
        .await
        .map(|verdict| verdict.ready)
}

/// [`automated_session_ready_for`], with the item that disagreed.
pub async fn automated_session_readiness_for(
    target: &ComputeTarget,
    expected_user: &str,
    password: Option<&str>,
    runner: &Runner,
) -> Result<SessionReadiness, DeployError> {
    session_readiness_for(target, expected_user, "gui-ready", password, runner).await
}

/// Whether the signed helper can read this exact user's Apple challenge.
pub async fn apple_challenge_session_ready_for(
    target: &ComputeTarget,
    expected_user: &str,
    password: Option<&str>,
    runner: &Runner,
) -> Result<bool, DeployError> {
    apple_challenge_session_readiness_for(target, expected_user, password, runner)
        .await
        .map(|verdict| verdict.ready)
}

/// [`apple_challenge_session_ready_for`], with the item that disagreed.
pub async fn apple_challenge_session_readiness_for(
    target: &ComputeTarget,
    expected_user: &str,
    password: Option<&str>,
    runner: &Runner,
) -> Result<SessionReadiness, DeployError> {
    session_readiness_for(
        target,
        expected_user,
        "apple-challenge-ready",
        password,
        runner,
    )
    .await
}
