use super::*;

pub(in crate::deploy::host_gui_automation) async fn reconcile_apple_challenge_helper(
    target: &ComputeTarget,
    items: &mut Vec<(String, String)>,
    password: Option<&str>,
    runner: &Runner,
) -> Result<HelperIdentity, DeployError> {
    require_target(target)?;
    let path = apple_challenge_helper_path();
    if let Ok(Some(identity)) = helper_identity(target, path, runner).await {
        if identity.version == APPLE_CHALLENGE_HELPER_VERSION {
            items.push(("apple-challenge-helper".to_string(), "reused".to_string()));
            items.push((
                "apple-challenge-helper-version".to_string(),
                identity.version.clone(),
            ));
            return Ok(identity);
        }
    }

    let home = host_channel::remote_home(target, runner).await?;
    let signer = signing_program(target, &home, runner).await?;
    items.push(("apple-challenge-signer".to_string(), signer.clone()));
    let cache = format!("{home}/.stado/cache/apple-challenge-helper");
    let source = format!("{cache}/capture.swift");
    let staged = format!("{cache}/stado-apple-challenge-capture.staged");
    run(
        target,
        &["/bin/mkdir", "-p", &cache],
        "create Apple challenge helper cache",
        runner,
    )
    .await?;
    remove_if_present(target, &source, false, runner).await?;
    remove_if_present(target, &staged, false, runner).await?;
    let output_argument = format!("of={source}");
    let source_write = host_channel::run_program_with_stdin(
        target,
        &["/bin/dd", &output_argument, "bs=65536"],
        APPLE_CHALLENGE_HELPER_SOURCE,
        runner,
    )
    .await?;
    if !source_write.ok() {
        return Err(DeployError(format!(
            "{}: writing the Apple challenge helper source failed: {}",
            target.name,
            source_write.detail().trim()
        )));
    }
    run(
        target,
        &["/bin/chmod", "600", &source],
        "protect Apple challenge helper source",
        runner,
    )
    .await?;
    run(
        target,
        &["/usr/bin/xcrun", "swiftc", "-O", &source, "-o", &staged],
        "compile Apple challenge helper",
        runner,
    )
    .await?;
    run(
        target,
        &[
            &signer,
            "signing",
            "sign",
            "--identifier",
            APPLE_CHALLENGE_HELPER_BUNDLE_ID,
            "--previous",
            path,
            &staged,
        ],
        "sign Apple challenge helper",
        runner,
    )
    .await?;
    run_sudo(
        target,
        &["/bin/mkdir", "-p", "/usr/local/libexec"],
        "create the system helper directory",
        password,
        runner,
    )
    .await?;
    run_sudo(
        target,
        &[
            "/usr/bin/install",
            "-o",
            "root",
            "-g",
            "wheel",
            "-m",
            "755",
            &staged,
            path,
        ],
        "install Apple challenge helper",
        password,
        runner,
    )
    .await?;
    remove_if_present(target, &source, false, runner).await?;
    remove_if_present(target, &staged, false, runner).await?;

    let identity = helper_identity(target, path, runner)
        .await?
        .ok_or_else(|| DeployError("Apple challenge helper was not installed".to_string()))?;
    if identity.version != APPLE_CHALLENGE_HELPER_VERSION {
        return Err(DeployError(format!(
            "Apple challenge helper is version {}, expected {}",
            identity.version, APPLE_CHALLENGE_HELPER_VERSION
        )));
    }
    items.push((
        "apple-challenge-helper".to_string(),
        "installed".to_string(),
    ));
    items.push((
        "apple-challenge-helper-version".to_string(),
        identity.version.clone(),
    ));
    Ok(identity)
}

async fn signing_program(
    target: &ComputeTarget,
    home: &str,
    runner: &Runner,
) -> Result<String, DeployError> {
    let lookup =
        host_channel::run_program(target, &["/usr/bin/which", "wisent-products"], runner).await?;
    if lookup.ok() && !lookup.stdout.trim().is_empty() {
        return Ok(lookup.stdout.trim().to_string());
    }
    // pipx exposes the shared signer here even when SSH's PATH omits it.
    let installed = format!("{home}/.local/bin/wisent-products");
    if host_channel::remote_test(target, &format!("-x {}", shlex_quote(&installed)), runner).await? {
        return Ok(installed);
    }
    Err(DeployError(format!(
        "{}: Wisent Products signing executable is unavailable: PATH lookup returned {}; \
         {installed} is not executable. Install the shared signer on this build host",
        target.name,
        lookup.detail().trim()
    )))
}
