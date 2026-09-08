use super::*;

async fn rollback_app(
    target: &ComputeTarget,
    backup: &str,
    runner: &Runner,
) -> Result<(), DeployError> {
    let _ = run_sudo(
        target,
        &["/bin/rm", "-rf", CUA_DRIVER_APP],
        "remove failed CuaDriver install",
        None,
        runner,
    )
    .await;
    if host_channel::remote_test(
        target,
        &format!("-e {}", super::shlex_quote(backup)),
        runner,
    )
    .await?
    {
        run_sudo(
            target,
            &["/bin/mv", backup, CUA_DRIVER_APP],
            "restore prior CuaDriver app",
            None,
            runner,
        )
        .await?;
    }
    Ok(())
}

pub(in crate::deploy::host_gui_automation) async fn reconcile_app(
    target: &ComputeTarget,
    items: &mut Vec<(String, String)>,
    runner: &Runner,
) -> Result<(), DeployError> {
    require_target(target)?;
    if let Some(identity) = app_identity(target, CUA_DRIVER_APP, runner).await? {
        if identity.bundle == CUA_DRIVER_BUNDLE_ID && identity.version == CUA_DRIVER_VERSION {
            items.push(("cua-driver-app".to_string(), "reused".to_string()));
            items.push(("cua-driver-version".to_string(), identity.version));
            return Ok(());
        }
    }

    let home = host_channel::remote_home(target, runner).await?;
    let cache = format!("{home}/.stado/cache/cua-driver/{CUA_DRIVER_VERSION}");
    let archive = format!("{cache}/cua-driver.tar.gz");
    let partial = format!("{archive}.partial");
    let stage = format!("{cache}/stage");
    let stage_app =
        format!("{stage}/cua-driver-rs-{CUA_DRIVER_VERSION}-darwin-universal/CuaDriver.app");
    let backup = format!("{CUA_DRIVER_APP}.stado-backup");

    run(
        target,
        &["/bin/mkdir", "-p", &cache],
        "create CuaDriver cache",
        runner,
    )
    .await?;
    let archive_valid = if host_channel::remote_test(
        target,
        &format!("-f {}", super::shlex_quote(&archive)),
        runner,
    )
    .await?
    {
        let digest = run(
            target,
            &["/usr/bin/openssl", "dgst", "-sha256", "-r", &archive],
            "cached CuaDriver digest",
            runner,
        )
        .await?
        .stdout;
        digest.split_whitespace().next() == Some(CUA_DRIVER_ARCHIVE_SHA256)
    } else {
        false
    };
    if !archive_valid {
        // The host channel has a bounded command window. Keep a verified
        // version-scoped partial and resume it on the next reconciliation;
        // deleting it first makes every slow GitHub download restart at byte
        // zero and therefore guarantees the same timeout forever.
        run(
            target,
            &[
                "/usr/bin/curl",
                "-fL",
                "--retry",
                "3",
                "--continue-at",
                "-",
                "--output",
                &partial,
                CUA_DRIVER_ARCHIVE_URL,
            ],
            "download pinned CuaDriver release",
            runner,
        )
        .await?;
        let digest = run(
            target,
            &["/usr/bin/openssl", "dgst", "-sha256", "-r", &partial],
            "downloaded CuaDriver digest",
            runner,
        )
        .await?
        .stdout;
        if digest.split_whitespace().next() != Some(CUA_DRIVER_ARCHIVE_SHA256) {
            remove_if_present(target, &partial, false, runner).await?;
            return Err(DeployError(
                "downloaded CuaDriver archive digest does not match the pinned release".to_string(),
            ));
        }
        run(
            target,
            &["/bin/mv", &partial, &archive],
            "commit CuaDriver archive",
            runner,
        )
        .await?;
    }

    remove_if_present(target, &stage, false, runner).await?;
    run(
        target,
        &["/bin/mkdir", "-p", &stage],
        "create CuaDriver staging directory",
        runner,
    )
    .await?;
    run(
        target,
        &["/usr/bin/tar", "-xzf", &archive, "-C", &stage],
        "extract CuaDriver release",
        runner,
    )
    .await?;
    let staged = app_identity(target, &stage_app, runner)
        .await?
        .ok_or_else(|| DeployError("CuaDriver release contains no app bundle".to_string()))?;
    if staged.bundle != CUA_DRIVER_BUNDLE_ID || staged.version != CUA_DRIVER_VERSION {
        return Err(DeployError(format!(
            "CuaDriver release identity is {} {}, expected {} {}",
            staged.bundle, staged.version, CUA_DRIVER_BUNDLE_ID, CUA_DRIVER_VERSION
        )));
    }

    if host_channel::remote_test(
        target,
        &format!("-e {}", super::shlex_quote(&backup)),
        runner,
    )
    .await?
    {
        if let Some(installed) = app_identity(target, CUA_DRIVER_APP, runner).await? {
            if installed.bundle == CUA_DRIVER_BUNDLE_ID && installed.version == CUA_DRIVER_VERSION {
                remove_if_present(target, &backup, true, runner).await?;
            } else {
                rollback_app(target, &backup, runner).await?;
            }
        } else {
            rollback_app(target, &backup, runner).await?;
        }
    }
    if host_channel::remote_test(
        target,
        &format!("-e {}", super::shlex_quote(CUA_DRIVER_APP)),
        runner,
    )
    .await?
    {
        run_sudo(
            target,
            &["/bin/mv", CUA_DRIVER_APP, &backup],
            "back up prior CuaDriver app",
            None,
            runner,
        )
        .await?;
    }
    if let Err(error) = run_sudo(
        target,
        &["/usr/bin/ditto", &stage_app, CUA_DRIVER_APP],
        "install CuaDriver app",
        None,
        runner,
    )
    .await
    {
        rollback_app(target, &backup, runner).await?;
        return Err(error);
    }
    let installed = match app_identity(target, CUA_DRIVER_APP, runner).await {
        Ok(Some(identity)) => identity,
        Ok(None) => {
            rollback_app(target, &backup, runner).await?;
            return Err(DeployError(
                "installed CuaDriver app is missing".to_string(),
            ));
        }
        Err(error) => {
            rollback_app(target, &backup, runner).await?;
            return Err(error);
        }
    };
    if installed != staged {
        rollback_app(target, &backup, runner).await?;
        return Err(DeployError(
            "installed CuaDriver app did not preserve its signed identity".to_string(),
        ));
    }
    if let Err(error) = run(
        target,
        &[LSREGISTER, "-f", CUA_DRIVER_APP],
        "register CuaDriver with LaunchServices",
        runner,
    )
    .await
    {
        rollback_app(target, &backup, runner).await?;
        return Err(error);
    }
    remove_if_present(target, &backup, true, runner).await?;
    let bin_dir = format!("{home}/.local/bin");
    let bin_link = format!("{bin_dir}/cua-driver");
    let app_binary = format!("{CUA_DRIVER_APP}/Contents/MacOS/cua-driver");
    run(
        target,
        &["/bin/mkdir", "-p", &bin_dir],
        "create CuaDriver binary directory",
        runner,
    )
    .await?;
    run(
        target,
        &["/bin/ln", "-sfn", &app_binary, &bin_link],
        "publish CuaDriver binary link",
        runner,
    )
    .await?;
    remove_if_present(target, &stage, false, runner).await?;
    items.push(("cua-driver-app".to_string(), "installed".to_string()));
    items.push((
        "cua-driver-version".to_string(),
        CUA_DRIVER_VERSION.to_string(),
    ));
    items.push((
        "cua-driver-sha256".to_string(),
        CUA_DRIVER_ARCHIVE_SHA256.to_string(),
    ));
    Ok(())
}
