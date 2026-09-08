use super::*;

/// Capture one Apple trusted-device code inside the verified GUI user's Aqua
/// session. The code exists only in an owner-only file on that host and in the
/// returned in-memory value; diagnostics never include it.
pub async fn capture_apple_challenge(
    target: &ComputeTarget,
    expected_user: &str,
    capture_id: &str,
    wait_seconds: u64,
    password: Option<&str>,
    runner: &Runner,
) -> Result<String, DeployError> {
    safe_identity(expected_user, "GUI user")?;
    safe_identity(capture_id, "Apple challenge capture id")?;
    if !(1..=90).contains(&wait_seconds) {
        return Err(DeployError(
            "Apple challenge wait must be between 1 and 90 seconds".to_string(),
        ));
    }
    let session = preflight_apple_challenge(target, expected_user, password, runner).await?;
    let user = session.user;
    let uid = session.uid;

    let work = format!("/Users/{user}/.stado/work/apple-challenge");
    let output_file = format!("{work}/{capture_id}.code");
    run_in_gui_session(
        target,
        &user,
        &uid,
        &["/bin/mkdir", "-p", &work],
        "create Apple challenge work directory",
        password,
        runner,
    )
    .await?;
    run_in_gui_session(
        target,
        &user,
        &uid,
        &["/bin/chmod", "700", &work],
        "protect Apple challenge work directory",
        password,
        runner,
    )
    .await?;
    run_in_gui_session(
        target,
        &user,
        &uid,
        &["/bin/rm", "-f", &output_file],
        "remove stale Apple challenge file",
        password,
        runner,
    )
    .await?;

    let wait = wait_seconds.to_string();
    let capture = invoke_in_gui_session(
        target,
        &user,
        &uid,
        &[
            apple_challenge_helper_path(),
            "--output-file",
            &output_file,
            "--click-allow",
            "--click-done",
            "--wait-seconds",
            &wait,
        ],
        password,
        runner,
    )
    .await?;
    if !capture.ok() {
        let _ = run_in_gui_session(
            target,
            &user,
            &uid,
            &["/bin/rm", "-f", &output_file],
            "remove failed Apple challenge file",
            password,
            runner,
        )
        .await;
        return Err(DeployError(format!(
            "{} could not capture the Apple challenge: {}",
            target.name,
            capture.detail().trim()
        )));
    }

    let mut captured = invoke_in_gui_session(
        target,
        &user,
        &uid,
        &["/bin/cat", &output_file],
        password,
        runner,
    )
    .await?;
    let cleanup = run_in_gui_session(
        target,
        &user,
        &uid,
        &["/bin/rm", "-f", &output_file],
        "remove consumed Apple challenge file",
        password,
        runner,
    )
    .await;
    if !captured.ok() {
        return Err(DeployError(format!(
            "{} captured an Apple challenge but could not read its protected file",
            target.name
        )));
    }
    cleanup?;
    let code = captured.stdout.trim().to_string();
    captured.stdout.clear();
    if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(DeployError(format!(
            "{} returned an invalid Apple challenge",
            target.name
        )));
    }
    Ok(code)
}
