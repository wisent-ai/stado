use super::*;

mod autologin;
mod install;
mod runtime;

pub(in crate::deploy::host_gui_automation) use autologin::reconcile_autologin;
pub(in crate::deploy::host_gui_automation) use install::reconcile_app;
pub(in crate::deploy::host_gui_automation) use runtime::reconcile_runtime;

async fn session_ready_for(
    target: &ComputeTarget,
    expected_user: &str,
    readiness_key: &str,
    password: Option<&str>,
    runner: &Runner,
) -> Result<bool, DeployError> {
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
    Ok(value("console") == Some(expected_user)
        && value("accessibility-user") == Some(expected_user)
        && value(readiness_key) == Some("yes"))
}

/// Whether CuaDriver can drive this exact user's current GUI session.
pub async fn automated_session_ready_for(
    target: &ComputeTarget,
    expected_user: &str,
    password: Option<&str>,
    runner: &Runner,
) -> Result<bool, DeployError> {
    session_ready_for(target, expected_user, "gui-ready", password, runner).await
}

/// Whether the signed helper can read this exact user's Apple challenge.
pub async fn apple_challenge_session_ready_for(
    target: &ComputeTarget,
    expected_user: &str,
    password: Option<&str>,
    runner: &Runner,
) -> Result<bool, DeployError> {
    session_ready_for(
        target,
        expected_user,
        "apple-challenge-ready",
        password,
        runner,
    )
    .await
}
