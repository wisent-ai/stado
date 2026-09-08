use super::*;

mod capture;
mod install;

pub use capture::capture_apple_challenge;
pub(in crate::deploy::host_gui_automation) use install::reconcile_apple_challenge_helper;

/// Exercise the exact signed AX client in the exact Aqua session without
/// scanning windows or opening a system prompt.
pub(crate) async fn preflight_apple_challenge(
    target: &ComputeTarget,
    expected_user: &str,
    password: Option<&str>,
    runner: &Runner,
) -> Result<AppleChallengeSession, DeployError> {
    safe_identity(expected_user, "GUI user")?;
    require_declared_session(target, expected_user)?;
    let helper = helper_identity(target, apple_challenge_helper_path(), runner)
        .await?
        .ok_or_else(|| DeployError("Apple challenge helper is not installed".to_string()))?;
    if helper.version != APPLE_CHALLENGE_HELPER_VERSION {
        return Err(DeployError(format!(
            "Apple challenge helper is version {}, expected {}",
            helper.version, APPLE_CHALLENGE_HELPER_VERSION
        )));
    }
    let user = run(
        target,
        &["/usr/bin/stat", "-f", "%Su", "/dev/console"],
        "read the Apple challenge console user",
        runner,
    )
    .await?
    .stdout
    .trim()
    .to_string();
    if user != expected_user {
        return Err(DeployError(format!(
            "{} has console user {user}, not {expected_user}",
            target.name
        )));
    }
    let uid = gui_user_id(target, &user, runner).await?;
    let output = invoke_in_gui_session(
        target,
        &user,
        &uid,
        &[apple_challenge_helper_path(), "--preflight"],
        password,
        runner,
    )
    .await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: the Apple challenge helper cannot use Accessibility in {user}'s Aqua session: {}",
            target.name,
            output.detail().trim()
        )));
    }
    let report: serde_json::Value =
        serde_json::from_str(output.stdout.trim()).map_err(|error| {
            DeployError(format!(
                "{}: Apple challenge preflight returned invalid JSON: {error}",
                target.name
            ))
        })?;
    if report.get("version").and_then(serde_json::Value::as_str)
        != Some(APPLE_CHALLENGE_HELPER_VERSION)
        || report.get("ok").and_then(serde_json::Value::as_bool) != Some(true)
        || report
            .get("accessibilityTrusted")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
    {
        return Err(DeployError(format!(
            "{}: Apple challenge preflight did not confirm Accessibility",
            target.name
        )));
    }
    Ok(AppleChallengeSession { user, uid })
}
