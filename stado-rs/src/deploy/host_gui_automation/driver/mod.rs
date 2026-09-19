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
        return Err(DeployError(error));
    }
    let value = |key: &str| {
        report
            .items
            .iter()
            .find_map(|(name, value)| (name == key).then_some(value.as_str()))
    };
    let mismatches: Vec<String> = [
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
    Ok(SessionReadiness {
        ready: mismatches.is_empty(),
        reason: (!mismatches.is_empty()).then(|| mismatches.join("; ")),
    })
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
