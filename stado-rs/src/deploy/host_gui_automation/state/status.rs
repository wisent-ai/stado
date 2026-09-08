use super::*;

pub(in crate::deploy::host_gui_automation) async fn status_inner(
    target: &ComputeTarget,
    items: &mut Vec<(String, String)>,
    password: Option<&str>,
    runner: &Runner,
) -> Result<(), DeployError> {
    require_target(target)?;
    let autologin = optional_sudo(
        target,
        &[
            "/usr/bin/defaults",
            "read",
            "/Library/Preferences/com.apple.loginwindow",
            "autoLoginUser",
        ],
        password,
        runner,
    )
    .await?
    .filter(|value| !value.is_empty())
    .unwrap_or_else(|| "none".to_string());
    items.push(("autologin".to_string(), autologin));
    let kcpassword = optional_sudo(
        target,
        &["/bin/test", "-f", "/etc/kcpassword"],
        password,
        runner,
    )
    .await?
    .is_some();
    items.push((
        "kcpassword".to_string(),
        if kcpassword { "present" } else { "absent" }.to_string(),
    ));
    let ard = optional_sudo(
        target,
        &[
            "/usr/bin/defaults",
            "read",
            REMOTE_MANAGEMENT_PREFS,
            "ARD_AllLocalUsers",
        ],
        password,
        runner,
    )
    .await?
    .filter(|value| !value.is_empty())
    .unwrap_or_else(|| "unset".to_string());
    items.push(("remote-management-all-users".to_string(), ard));
    let vnc = optional_sudo(
        target,
        &[
            "/usr/bin/defaults",
            "read",
            REMOTE_MANAGEMENT_PREFS,
            "VNCLegacyConnectionsEnabled",
        ],
        password,
        runner,
    )
    .await?
    .filter(|value| !value.is_empty())
    .unwrap_or_else(|| "unset".to_string());
    items.push(("vnc-legacy".to_string(), vnc));
    let console = optional(
        target,
        &["/usr/bin/stat", "-f", "%Su", "/dev/console"],
        runner,
    )
    .await?
    .unwrap_or_else(|| "unknown".to_string());
    items.push(("console".to_string(), console.clone()));

    let user = login_user(target, runner).await?;
    let database = format!("/Users/{user}/Library/Application Support/com.apple.TCC/TCC.db");
    let identity = app_identity(target, CUA_DRIVER_APP, runner).await?;
    if let Some(identity) = &identity {
        items.push(("cua-driver-app".to_string(), "present".to_string()));
        items.push(("cua-driver-version".to_string(), identity.version.clone()));
        items.push(("cua-driver-client".to_string(), identity.bundle.clone()));
    } else {
        items.push(("cua-driver-app".to_string(), "absent".to_string()));
    }

    let helper = helper_identity(target, apple_challenge_helper_path(), runner).await?;
    if let Some(helper) = &helper {
        items.push(("apple-challenge-helper".to_string(), "present".to_string()));
        items.push((
            "apple-challenge-helper-version".to_string(),
            helper.version.clone(),
        ));
    } else {
        items.push(("apple-challenge-helper".to_string(), "absent".to_string()));
    }

    let accessibility = if let Some(identity) = &identity {
        let query = format!(
            "SELECT COUNT(*) FROM access WHERE service = '{ACCESSIBILITY_SERVICE}' \
             AND auth_value = 2 AND ((client = '{}' AND client_type = 0) \
             OR (client = '{CUA_DRIVER_EXECUTABLE}' AND client_type = 1));",
            identity.bundle
        );
        let value = run_sudo(
            target,
            &["/usr/bin/sqlite3", &database, &query],
            "read CuaDriver Accessibility",
            password,
            runner,
        )
        .await?
        .stdout;
        match value.trim() {
            "2" => "granted".to_string(),
            "0" | "1" | "" => "not-set".to_string(),
            other => format!("refused:{other}"),
        }
    } else {
        "app-missing".to_string()
    };
    items.push(("accessibility-record".to_string(), accessibility));

    let challenge_accessibility = if helper.is_some() {
        let query = format!(
            "SELECT COUNT(*) FROM access WHERE service = '{ACCESSIBILITY_SERVICE}' \
             AND auth_value = 2 AND client = '{}' AND client_type = 1;",
            apple_challenge_helper_path()
        );
        let value = run_sudo(
            target,
            &["/usr/bin/sqlite3", &database, &query],
            "read Apple challenge Accessibility",
            password,
            runner,
        )
        .await?
        .stdout;
        match value.trim() {
            "1" => "granted".to_string(),
            "0" | "" => "not-set".to_string(),
            other => format!("refused:{other}"),
        }
    } else {
        "helper-missing".to_string()
    };
    items.push((
        "apple-challenge-accessibility".to_string(),
        challenge_accessibility.clone(),
    ));
    items.push(("accessibility-user".to_string(), user.clone()));

    let uid = gui_user_id(target, &user, runner).await?;
    let qualified = format!("gui/{uid}/{CUA_DRIVER_RUNTIME_LABEL}");
    let runtime = if invoke_as_gui_user(
        target,
        &user,
        &["/bin/launchctl", "print", &qualified],
        password,
        runner,
    )
    .await?
    .ok()
    {
        "running"
    } else {
        "absent"
    };
    let socket = format!("/Users/{user}/Library/Caches/cua-driver/probierz.sock");
    let socket_ready = invoke_as_gui_user(
        target,
        &user,
        &["/bin/test", "-S", &socket],
        password,
        runner,
    )
    .await?
    .ok();
    items.push(("cua-driver-runtime".to_string(), runtime.to_string()));
    items.push((
        "cua-driver-socket".to_string(),
        if socket_ready { "ready" } else { "absent" }.to_string(),
    ));

    let permission = if socket_ready {
        match invoke_as_gui_user(
            target,
            &user,
            &[
                CUA_DRIVER_EXECUTABLE,
                "call",
                "check_permissions",
                r#"{"prompt":false}"#,
                "--socket",
                &socket,
            ],
            password,
            runner,
        )
        .await
        {
            Ok(output) if output.ok() => {
                serde_json::from_str::<serde_json::Value>(output.stdout.trim())
                    .map_err(|error| {
                        format!("CuaDriver check_permissions returned invalid JSON: {error}")
                    })
                    .and_then(|value| {
                        value
                            .get("accessibility")
                            .and_then(serde_json::Value::as_bool)
                            .ok_or_else(|| {
                                format!(
                                "CuaDriver check_permissions returned no accessibility boolean: {}",
                                output.stdout.trim()
                            )
                            })
                    })
            }
            Ok(output) => Err(format!(
                "CuaDriver check_permissions failed: {}",
                output.detail().trim()
            )),
            Err(error) => Err(format!(
                "CuaDriver check_permissions could not run: {error}"
            )),
        }
    } else {
        Err(format!("CuaDriver socket is absent: {socket}"))
    };
    let observed_accessibility = match permission {
        Ok(true) => "granted",
        Ok(false) => "denied",
        Err(detail) => {
            items.push(("accessibility-error".to_string(), detail));
            "unobserved"
        }
    };
    items.push((
        "accessibility".to_string(),
        observed_accessibility.to_string(),
    ));

    let console_ready = !matches!(console.as_str(), "" | "root" | "loginwindow" | "unknown");
    // Whose session this is belongs in the readiness answer, not beside it. A host can
    // hold a driver, grants and a live socket in one user's session while the identity
    // the fleet placed here lives in another.
    for (named, held) in declared_gui_bindings(target) {
        items.push((format!("identity-user:{held}"), named));
    }
    let declared_session = automates_declared_session(target, &user);
    items.push((
        "automated-session-declared".to_string(),
        if declared_session { "yes" } else { "no" }.to_string(),
    ));
    let gui_ready = console_ready
        && declared_session
        && observed_accessibility == "granted"
        && runtime == "running"
        && socket_ready;
    items.push((
        "gui-ready".to_string(),
        if gui_ready { "yes" } else { "no" }.to_string(),
    ));
    let challenge_ready = if console_ready
        && declared_session
        && helper
            .as_ref()
            .is_some_and(|value| value.version == APPLE_CHALLENGE_HELPER_VERSION)
        && challenge_accessibility == "granted"
    {
        match preflight_apple_challenge(target, &user, password, runner).await {
            Ok(_) => true,
            Err(error) => {
                items.push(("apple-challenge-preflight-error".to_string(), error.0));
                false
            }
        }
    } else {
        false
    };
    items.push((
        "apple-challenge-ready".to_string(),
        if challenge_ready { "yes" } else { "no" }.to_string(),
    ));
    Ok(())
}
