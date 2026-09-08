use super::*;

pub(in crate::deploy::host_gui_automation) async fn disable_inner(
    target: &ComputeTarget,
    bundle: &str,
    items: &mut Vec<(String, String)>,
    runner: &Runner,
) -> Result<(), DeployError> {
    require_target(target)?;
    let user = login_user(target, runner).await?;
    let uid = gui_user_id(target, &user, runner).await?;
    let home = format!("/Users/{user}");
    let command_home = host_channel::remote_home(target, runner).await?;

    if optional_sudo(
        target,
        &[
            "/usr/bin/defaults",
            "read",
            "/Library/Preferences/com.apple.loginwindow",
            "autoLoginUser",
        ],
        None,
        runner,
    )
    .await?
    .is_some()
    {
        let _ = run_sudo(
            target,
            &[
                "/usr/bin/defaults",
                "delete",
                "/Library/Preferences/com.apple.loginwindow",
                "autoLoginUser",
            ],
            "clear autologin",
            None,
            runner,
        )
        .await;
        items.push(("autologin".to_string(), "removed".to_string()));
    } else {
        items.push(("autologin".to_string(), "absent".to_string()));
    }
    if run_sudo(
        target,
        &["/bin/test", "-f", "/etc/kcpassword"],
        "read kcpassword state",
        None,
        runner,
    )
    .await
    .is_ok()
    {
        run_sudo(
            target,
            &["/bin/rm", "-f", "/etc/kcpassword"],
            "remove kcpassword",
            None,
            runner,
        )
        .await?;
        items.push(("kcpassword".to_string(), "removed".to_string()));
    } else {
        items.push(("kcpassword".to_string(), "absent".to_string()));
    }

    let _ = run_sudo(
        target,
        &[KICKSTART, "-deactivate", "-configure", "-access", "-off"],
        "deactivate Remote Management",
        None,
        runner,
    )
    .await;
    let _ = run_sudo(
        target,
        &[
            KICKSTART,
            "-configure",
            "-clientopts",
            "-setvnclegacy",
            "-vnclegacy",
            "no",
        ],
        "disable legacy VNC",
        None,
        runner,
    )
    .await;
    for key in [
        "ARD_AllLocalUsers",
        "ARD_AllLocalUsersPrivs",
        "VNCLegacyConnectionsEnabled",
    ] {
        let _ = run_sudo(
            target,
            &["/usr/bin/defaults", "delete", REMOTE_MANAGEMENT_PREFS, key],
            "clear Remote Management preference",
            None,
            runner,
        )
        .await;
    }
    items.push(("remote-management".to_string(), "deactivated".to_string()));

    if !bundle.is_empty() {
        safe_identity(bundle, "bundle identifier")?;
    }
    let database = format!("{home}/Library/Application Support/com.apple.TCC/TCC.db");
    let bundle_clause = if bundle.is_empty() {
        String::new()
    } else {
        format!(" OR (client = '{bundle}' AND client_type = 0)")
    };
    let sql = format!(
        "DELETE FROM access WHERE service = '{ACCESSIBILITY_SERVICE}' AND \
         ((client = '{CUA_DRIVER_EXECUTABLE}' AND client_type = 1) \
         OR (client = '{}' AND client_type = 1){bundle_clause});",
        apple_challenge_helper_path()
    );
    run_sudo(
        target,
        &["/usr/bin/sqlite3", &database, &sql],
        "revoke GUI automation Accessibility",
        None,
        runner,
    )
    .await?;
    items.push((
        "tcc-revoked".to_string(),
        "CuaDriver and Apple challenge helper".to_string(),
    ));

    for label in [CUA_DRIVER_RUNTIME_LABEL, LEGACY_CUA_DRIVER_RUNTIME_LABEL] {
        let qualified = format!("gui/{uid}/{label}");
        let _ = invoke_as_gui_user(
            target,
            &user,
            &["/bin/launchctl", "bootout", &qualified],
            None,
            runner,
        )
        .await?;
    }
    for path in [
        format!("{home}/Library/LaunchAgents/{CUA_DRIVER_RUNTIME_LABEL}.plist"),
        format!("{home}/Library/Caches/cua-driver/probierz.sock"),
        format!("{home}/.local/bin/cua-driver"),
        format!("{home}/.stado/cache/cua-driver"),
        format!("{home}/.stado/cache/apple-challenge-helper"),
        format!("{home}/.stado/cache/gui-automation"),
    ] {
        run_as_gui_user(
            target,
            &user,
            &["/bin/rm", "-rf", &path],
            "remove GUI automation user state",
            None,
            runner,
        )
        .await?;
    }
    items.push(("cua-driver-runtime".to_string(), "removed".to_string()));

    remove_if_present(target, CUA_DRIVER_APP, true, runner).await?;
    remove_if_present(target, apple_challenge_helper_path(), true, runner).await?;
    if command_home != home {
        for path in [
            format!("{command_home}/.stado/cache/cua-driver"),
            format!("{command_home}/.stado/cache/apple-challenge-helper"),
            format!("{command_home}/.stado/cache/gui-automation"),
        ] {
            remove_if_present(target, &path, false, runner).await?;
        }
    }
    items.push(("cua-driver-app".to_string(), "removed".to_string()));
    items.push(("apple-challenge-helper".to_string(), "removed".to_string()));
    Ok(())
}
