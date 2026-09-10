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
    if host_channel::remote_test(target, &format!("-x {}", shlex_quote(&installed)), runner).await?
    {
        return Ok(installed);
    }
    bootstrap_signer(target, home, runner).await
}

async fn bootstrap_signer(
    target: &ComputeTarget,
    home: &str,
    runner: &Runner,
) -> Result<String, DeployError> {
    use base64::Engine;

    // Private build input from wisent-products 43f83a7, never a public release.
    const SOURCE_SHA256: &str = "01c9d50de40f7f6ca5fbbacabd5f4f1faa4f7b8c95d07a0e628832e6ad158259";
    let program = format!("{home}/.stado/cache/native-signing/{SOURCE_SHA256}/bin/wisent-products");
    if host_channel::remote_test(target, &format!("-x {}", shlex_quote(&program)), runner).await? {
        return Ok(program);
    }
    let namespace = crate::config::wc_stado_storage_namespace();
    if namespace.is_empty() {
        return Err(DeployError(
            "native signing runtime is absent and storage.stado.namespace is not configured".into(),
        ));
    }
    let uri = format!("stado://{namespace}/artifacts/native-signing/{SOURCE_SHA256}.tar.gz");
    let source = crate::cli::storage::fetch_object(&uri)
        .await
        .map_err(|error| DeployError(format!("cannot read native signing input {uri}: {error}")))?;
    if crate::release_control::sha256_bytes(&source) != SOURCE_SHA256 {
        return Err(DeployError(format!(
            "native signing input digest mismatch: {uri}"
        )));
    }
    let input = serde_json::json!({
        "archive": base64::engine::general_purpose::STANDARD.encode(source),
        "sha256": SOURCE_SHA256,
    });
    let output = host_channel::run_program_with_stdin(
        target,
        &[
            "/usr/bin/python3",
            "-c",
            include_str!("../../../host_payloads/native-signing-runtime.py"),
        ],
        &input.to_string(),
        runner,
    )
    .await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: native signing runtime preparation failed: {}",
            target.name,
            output.detail().trim()
        )));
    }
    let observed: serde_json::Value = serde_json::from_str(&output.stdout)
        .map_err(|error| DeployError(format!("invalid native signing runtime receipt: {error}")))?;
    if observed["program"].as_str() != Some(program.as_str())
        || observed["source_sha256"].as_str() != Some(SOURCE_SHA256)
    {
        return Err(DeployError(
            "native signing runtime returned another source or program".into(),
        ));
    }
    Ok(program)
}
