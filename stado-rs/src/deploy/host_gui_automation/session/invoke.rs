use super::*;

pub(in crate::deploy::host_gui_automation) async fn run(
    target: &ComputeTarget,
    words: &[&str],
    what: &str,
    runner: &Runner,
) -> Result<CommandOutput, DeployError> {
    let output = host_channel::run_program(target, words, runner).await?;
    if output.ok() {
        Ok(output)
    } else {
        Err(DeployError(format!(
            "{}: {what} failed: {}",
            target.name,
            output.detail().trim()
        )))
    }
}

async fn invoke_sudo(
    target: &ComputeTarget,
    words: &[&str],
    password: Option<&str>,
    runner: &Runner,
) -> Result<CommandOutput, DeployError> {
    let prefix: &[&str] = if password.is_some() {
        &["/usr/bin/sudo", "-S", "-p", ""]
    } else {
        &["/usr/bin/sudo", "-n"]
    };
    let mut command = Vec::with_capacity(prefix.len() + words.len());
    command.extend_from_slice(prefix);
    command.extend_from_slice(words);
    if let Some(password) = password {
        host_channel::run_program_with_stdin(target, &command, &format!("{password}\n"), runner)
            .await
    } else {
        host_channel::run_program(target, &command, runner).await
    }
}

pub(in crate::deploy::host_gui_automation) async fn run_sudo(
    target: &ComputeTarget,
    words: &[&str],
    what: &str,
    password: Option<&str>,
    runner: &Runner,
) -> Result<CommandOutput, DeployError> {
    let output = invoke_sudo(target, words, password, runner).await?;
    if output.ok() {
        Ok(output)
    } else {
        Err(DeployError(format!(
            "{}: {what} failed: {}",
            target.name,
            output.detail().trim()
        )))
    }
}

pub(in crate::deploy::host_gui_automation) async fn gui_user_id(
    target: &ComputeTarget,
    user: &str,
    runner: &Runner,
) -> Result<String, DeployError> {
    safe_identity(user, "GUI user")?;
    let uid = run(
        target,
        &["/usr/bin/id", "-u", user],
        "resolve GUI user id",
        runner,
    )
    .await?
    .stdout
    .trim()
    .to_string();
    if uid.is_empty() || !uid.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(DeployError(format!(
            "{} returned an invalid GUI user id: {uid}",
            target.name
        )));
    }
    Ok(uid)
}

pub(in crate::deploy::host_gui_automation) async fn invoke_in_gui_session(
    target: &ComputeTarget,
    user: &str,
    uid: &str,
    words: &[&str],
    password: Option<&str>,
    runner: &Runner,
) -> Result<CommandOutput, DeployError> {
    let mut command = Vec::with_capacity(words.len() + 8);
    command.extend([
        "/bin/launchctl",
        "asuser",
        uid,
        "/usr/bin/sudo",
        "-n",
        "-u",
        user,
        "--",
    ]);
    command.extend_from_slice(words);
    invoke_sudo(target, &command, password, runner).await
}

pub(in crate::deploy::host_gui_automation) async fn invoke_as_gui_user(
    target: &ComputeTarget,
    user: &str,
    words: &[&str],
    password: Option<&str>,
    runner: &Runner,
) -> Result<CommandOutput, DeployError> {
    let uid = gui_user_id(target, user, runner).await?;
    invoke_in_gui_session(target, user, &uid, words, password, runner).await
}

pub(in crate::deploy::host_gui_automation) async fn run_in_gui_session(
    target: &ComputeTarget,
    user: &str,
    uid: &str,
    words: &[&str],
    what: &str,
    password: Option<&str>,
    runner: &Runner,
) -> Result<CommandOutput, DeployError> {
    let output = invoke_in_gui_session(target, user, uid, words, password, runner).await?;
    if output.ok() {
        Ok(output)
    } else {
        Err(DeployError(format!(
            "{}: {what} failed for GUI user {user}: {}",
            target.name,
            output.detail().trim()
        )))
    }
}

pub(in crate::deploy::host_gui_automation) async fn run_as_gui_user(
    target: &ComputeTarget,
    user: &str,
    words: &[&str],
    what: &str,
    password: Option<&str>,
    runner: &Runner,
) -> Result<CommandOutput, DeployError> {
    let uid = gui_user_id(target, user, runner).await?;
    run_in_gui_session(target, user, &uid, words, what, password, runner).await
}

pub(in crate::deploy::host_gui_automation) async fn optional(
    target: &ComputeTarget,
    words: &[&str],
    runner: &Runner,
) -> Result<Option<String>, DeployError> {
    let output = host_channel::run_program(target, words, runner).await?;
    Ok(output.ok().then(|| output.stdout.trim().to_string()))
}

pub(in crate::deploy::host_gui_automation) async fn optional_sudo(
    target: &ComputeTarget,
    words: &[&str],
    password: Option<&str>,
    runner: &Runner,
) -> Result<Option<String>, DeployError> {
    let output = invoke_sudo(target, words, password, runner).await?;
    Ok(output.ok().then(|| output.stdout.trim().to_string()))
}

pub(in crate::deploy::host_gui_automation) async fn remove_if_present(
    target: &ComputeTarget,
    path: &str,
    privileged: bool,
    runner: &Runner,
) -> Result<(), DeployError> {
    if !host_channel::remote_test(target, &format!("-e {}", super::shlex_quote(path)), runner)
        .await?
    {
        return Ok(());
    }
    if privileged {
        run_sudo(
            target,
            &["/bin/rm", "-rf", path],
            "remove stale CuaDriver path",
            None,
            runner,
        )
        .await?;
    } else {
        run(
            target,
            &["/bin/rm", "-rf", path],
            "remove stale CuaDriver path",
            runner,
        )
        .await?;
    }
    Ok(())
}
