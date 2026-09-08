use super::*;

pub(in crate::deploy::host_gui_automation) async fn reconcile_runtime(
    target: &ComputeTarget,
    items: &mut Vec<(String, String)>,
    runner: &Runner,
) -> Result<(), DeployError> {
    require_target(target)?;
    let user = login_user(target, runner).await?;
    let uid = gui_user_id(target, &user, runner).await?;
    let home = format!("/Users/{user}");
    let launch_agents = format!("{home}/Library/LaunchAgents");
    let caches = format!("{home}/Library/Caches/cua-driver");
    let logs = format!("{home}/.stado/logs");
    let plist = format!("{launch_agents}/{CUA_DRIVER_RUNTIME_LABEL}.plist");
    let staged = format!("{plist}.stado");
    let socket = format!("{caches}/probierz.sock");
    let stdout = format!("{logs}/probierz-cua-driver.out");
    let stderr = format!("{logs}/probierz-cua-driver.err");
    // LaunchServices supplies the WindowServer/Aqua responsibility chain that
    // AppKit (including NSPasteboard and Accessibility) requires. Starting the
    // Mach-O directly from launchd leaves those APIs unavailable.
    let arguments = serde_json::to_string(&[
        "/usr/bin/open",
        "-n",
        "-g",
        "-a",
        "CuaDriver",
        "--args",
        "serve",
        "--socket",
        socket.as_str(),
        "--no-permissions-gate",
    ])
    .map_err(|error| DeployError(format!("cannot encode CuaDriver arguments: {error}")))?;

    run_as_gui_user(
        target,
        &user,
        &["/bin/mkdir", "-p", &launch_agents, &caches, &logs],
        "create CuaDriver runtime directories",
        None,
        runner,
    )
    .await?;
    run_as_gui_user(
        target,
        &user,
        &["/bin/rm", "-f", &staged],
        "remove stale CuaDriver LaunchAgent staging file",
        None,
        runner,
    )
    .await?;
    run_as_gui_user(
        target,
        &user,
        &["/usr/bin/plutil", "-create", "xml1", &staged],
        "create CuaDriver LaunchAgent",
        None,
        runner,
    )
    .await?;
    for (key, value) in [
        ("Label", CUA_DRIVER_RUNTIME_LABEL),
        ("ProgramArguments", arguments.as_str()),
        ("LimitLoadToSessionType", "Aqua"),
        ("StandardOutPath", stdout.as_str()),
        ("StandardErrorPath", stderr.as_str()),
    ] {
        let kind = if key == "ProgramArguments" {
            "-json"
        } else {
            "-string"
        };
        run_as_gui_user(
            target,
            &user,
            &["/usr/bin/plutil", "-insert", key, kind, value, &staged],
            "write CuaDriver LaunchAgent",
            None,
            runner,
        )
        .await?;
    }
    run_as_gui_user(
        target,
        &user,
        &[
            "/usr/bin/plutil",
            "-insert",
            "RunAtLoad",
            "-bool",
            "true",
            &staged,
        ],
        "write CuaDriver LaunchAgent",
        None,
        runner,
    )
    .await?;
    run_as_gui_user(
        target,
        &user,
        &["/usr/bin/plutil", "-lint", &staged],
        "validate CuaDriver LaunchAgent",
        None,
        runner,
    )
    .await?;

    let qualified = format!("gui/{uid}/{CUA_DRIVER_RUNTIME_LABEL}");
    let definition_matches = invoke_as_gui_user(
        target,
        &user,
        &["/usr/bin/cmp", "-s", &staged, &plist],
        None,
        runner,
    )
    .await?
    .ok();
    let runtime_loaded = invoke_as_gui_user(
        target,
        &user,
        &["/bin/launchctl", "print", &qualified],
        None,
        runner,
    )
    .await?
    .ok();
    let socket_ready =
        invoke_as_gui_user(target, &user, &["/bin/test", "-S", &socket], None, runner)
            .await?
            .ok();
    if definition_matches && runtime_loaded && socket_ready {
        run_as_gui_user(
            target,
            &user,
            &["/bin/rm", "-f", &staged],
            "remove matched CuaDriver LaunchAgent staging file",
            None,
            runner,
        )
        .await?;
        items.push(("cua-driver-runtime".to_string(), "running".to_string()));
        items.push(("cua-driver-socket".to_string(), socket));
        return Ok(());
    }

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
    run_as_gui_user(
        target,
        &user,
        &["/bin/rm", "-f", &socket],
        "remove stale CuaDriver socket",
        None,
        runner,
    )
    .await?;
    run_as_gui_user(
        target,
        &user,
        &["/bin/mv", "-f", &staged, &plist],
        "install CuaDriver LaunchAgent",
        None,
        runner,
    )
    .await?;
    let domain = format!("gui/{uid}");
    run_as_gui_user(
        target,
        &user,
        &["/bin/launchctl", "bootstrap", &domain, &plist],
        "bootstrap CuaDriver LaunchAgent",
        None,
        runner,
    )
    .await?;
    let qualified = format!("{domain}/{CUA_DRIVER_RUNTIME_LABEL}");
    run_as_gui_user(
        target,
        &user,
        &["/bin/launchctl", "kickstart", "-k", &qualified],
        "start CuaDriver LaunchAgent",
        None,
        runner,
    )
    .await?;

    let mut socket_ready = false;
    for _ in 0..20 {
        if invoke_as_gui_user(target, &user, &["/bin/test", "-S", &socket], None, runner)
            .await?
            .ok()
        {
            socket_ready = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    if !socket_ready {
        return Err(DeployError(format!(
            "CuaDriver LaunchAgent started but did not create {socket}"
        )));
    }
    items.push(("cua-driver-runtime".to_string(), "running".to_string()));
    items.push(("cua-driver-socket".to_string(), socket));
    Ok(())
}
