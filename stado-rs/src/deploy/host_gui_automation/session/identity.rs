use super::*;

fn designated_requirement(output: &CommandOutput) -> Result<String, DeployError> {
    output
        .stderr
        .lines()
        .chain(output.stdout.lines())
        .find_map(|line| {
            line.split_once("designated => ")
                .map(|(_, value)| value.trim())
        })
        .map(str::to_string)
        .filter(|requirement| !requirement.is_empty())
        .ok_or_else(|| {
            DeployError(format!(
                "signed executable has no designated code requirement: {}",
                output.detail().trim()
            ))
        })
}

pub(in crate::deploy::host_gui_automation) async fn app_identity(
    target: &ComputeTarget,
    app: &str,
    runner: &Runner,
) -> Result<Option<AppIdentity>, DeployError> {
    if !host_channel::remote_test(target, &format!("-d {}", super::shlex_quote(app)), runner)
        .await?
    {
        return Ok(None);
    }
    run(
        target,
        &["/usr/bin/codesign", "--verify", "--deep", "--strict", app],
        "CuaDriver signature verification",
        runner,
    )
    .await?;
    let plist = format!("{app}/Contents/Info.plist");
    let bundle = run(
        target,
        &[PLIST_BUDDY, "-c", "Print :CFBundleIdentifier", &plist],
        "CuaDriver bundle identity read",
        runner,
    )
    .await?
    .stdout
    .trim()
    .to_string();
    safe_identity(&bundle, "bundle identifier")?;
    let version = run(
        target,
        &[
            PLIST_BUDDY,
            "-c",
            "Print :CFBundleShortVersionString",
            &plist,
        ],
        "CuaDriver version read",
        runner,
    )
    .await?
    .stdout
    .trim()
    .to_string();
    safe_identity(&version, "CuaDriver version")?;
    let requirement = designated_requirement(
        &run(
            target,
            &["/usr/bin/codesign", "-d", "-r-", app],
            "CuaDriver code requirement read",
            runner,
        )
        .await?,
    )?;
    Ok(Some(AppIdentity {
        bundle,
        version,
        requirement,
    }))
}

pub(in crate::deploy::host_gui_automation) fn apple_challenge_helper_path() -> &'static str {
    APPLE_CHALLENGE_HELPER
}

pub(in crate::deploy::host_gui_automation) async fn helper_identity(
    target: &ComputeTarget,
    path: &str,
    runner: &Runner,
) -> Result<Option<HelperIdentity>, DeployError> {
    if !host_channel::remote_test(target, &format!("-f {}", super::shlex_quote(path)), runner)
        .await?
    {
        return Ok(None);
    }
    run(
        target,
        &[
            "/usr/bin/codesign",
            "--verify",
            "--strict",
            "-R",
            "=anchor apple generic",
            path,
        ],
        "Apple challenge helper signature verification",
        runner,
    )
    .await?;
    let version_output = run(
        target,
        &[path, "--version"],
        "Apple challenge helper version read",
        runner,
    )
    .await?
    .stdout;
    let version = version_output
        .strip_prefix("stado-apple-challenge-capture ")
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    safe_identity(&version, "Apple challenge helper version")?;
    let requirement = designated_requirement(
        &run(
            target,
            &["/usr/bin/codesign", "-d", "-r-", path],
            "Apple challenge helper code requirement read",
            runner,
        )
        .await?,
    )?;
    Ok(Some(HelperIdentity {
        version,
        requirement,
    }))
}

pub(in crate::deploy::host_gui_automation) async fn login_user(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<String, DeployError> {
    let login = run(
        target,
        &["/usr/bin/id", "-un"],
        "remote login user read",
        runner,
    )
    .await?
    .stdout
    .trim()
    .to_string();
    let console = optional(
        target,
        &["/usr/bin/stat", "-f", "%Su", "/dev/console"],
        runner,
    )
    .await?
    .unwrap_or_default();
    let user = match console.as_str() {
        "" | "root" | "loginwindow" | "_mbsetupuser" => login,
        _ => console,
    };
    safe_identity(&user, "GUI user")?;
    if matches!(user.as_str(), "root" | "loginwindow" | "_mbsetupuser") {
        return Err(DeployError("the host has no non-root GUI user".to_string()));
    }
    Ok(user)
}
