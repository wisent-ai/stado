use super::*;

/// The CuaDriver runtime belongs to Probierz: its one process starts the
/// driver app on demand, through LaunchServices, and owns its socket. Stado
/// used to install a LaunchAgent of its own that did the same, which made
/// the driver a second unit of Probierz on the host and a second owner of
/// one socket. That unit is retired here; what Stado reconciles for the
/// driver is the app bundle and the Accessibility grant, and what it reports
/// is whether the socket Probierz owns is up.
pub(in crate::deploy::host_gui_automation) async fn reconcile_runtime(
    target: &ComputeTarget,
    items: &mut Vec<(String, String)>,
    runner: &Runner,
) -> Result<(), DeployError> {
    require_target(target)?;
    let user = login_user(target, runner).await?;
    let uid = gui_user_id(target, &user, runner).await?;
    let home = format!("/Users/{user}");
    let socket = format!("{home}/Library/Caches/cua-driver/probierz.sock");
    for label in [CUA_DRIVER_RUNTIME_LABEL, LEGACY_CUA_DRIVER_RUNTIME_LABEL] {
        let qualified = format!("gui/{uid}/{label}");
        if invoke_as_gui_user(
            target,
            &user,
            &["/bin/launchctl", "print", &qualified],
            None,
            runner,
        )
        .await?
        .ok()
        {
            run_as_gui_user(
                target,
                &user,
                &["/bin/launchctl", "bootout", &qualified],
                "retire the CuaDriver LaunchAgent; Probierz's own process runs the driver",
                None,
                runner,
            )
            .await?;
            items.push((format!("retired {label}"), "booted out".to_string()));
        }
        let plist = format!("{home}/Library/LaunchAgents/{label}.plist");
        run_as_gui_user(
            target,
            &user,
            &["/bin/rm", "-f", &plist, &format!("{plist}.stado")],
            "remove the retired CuaDriver LaunchAgent definition",
            None,
            runner,
        )
        .await?;
    }
    let socket_ready =
        invoke_as_gui_user(target, &user, &["/bin/test", "-S", &socket], None, runner)
            .await?
            .ok();
    items.push((
        "cua-driver-runtime".to_string(),
        if socket_ready {
            "running under Probierz's process".to_string()
        } else {
            "started by Probierz's process on its first desktop run".to_string()
        },
    ));
    items.push(("cua-driver-socket".to_string(), socket));
    Ok(())
}
