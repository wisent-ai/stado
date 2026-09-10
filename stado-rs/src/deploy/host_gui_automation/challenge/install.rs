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
    sign_helper(target, &signer, &staged, path, runner).await?;
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

/// Sign the staged helper with the fleet's stored Apple certificate.
///
/// A build host holds no signing identity of its own. The certificate and its
/// key live in Skarbiec, are read here, and reach the host on stdin; the signer
/// puts them in a temporary keychain it deletes again, so no host keychain and
/// no login item is changed and no system dialog is opened.
async fn sign_helper(
    target: &ComputeTarget,
    signer: &str,
    staged: &str,
    previous: &str,
    runner: &Runner,
) -> Result<(), DeployError> {
    let request = serde_json::json!({
        "program": signer,
        "identifier": APPLE_CHALLENGE_HELPER_BUNDLE_ID,
        "target": staged,
        "previous": previous,
        "certificate": signing_credential("certificate").await?,
        "private_key": signing_credential("private_key").await?,
    });
    let output = host_channel::run_program_with_stdin(
        target,
        &[
            "/usr/bin/python3",
            "-c",
            include_str!("../../../host_payloads/native_signing/sign.py"),
        ],
        &request.to_string(),
        runner,
    )
    .await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: sign Apple challenge helper failed: {}",
            target.name,
            output.detail().trim()
        )));
    }
    let report: serde_json::Value = serde_json::from_str(&output.stdout).map_err(|error| {
        DeployError(format!("invalid Apple challenge signing receipt: {error}"))
    })?;
    if report["state"].as_str() != Some("stable") {
        return Err(DeployError(format!(
            "{}: Apple challenge helper signature is {}",
            target.name, report["state"]
        )));
    }
    Ok(())
}

/// One field of the fleet's Apple signing certificate: the broker grant first,
/// then the owner vault, naming both failures rather than one.
async fn signing_credential(field: &str) -> Result<String, DeployError> {
    let broker = crate::credential_store::read_string(APPLE_SIGNING_CERTIFICATE_ITEM, field).await;
    if let Ok(Some(value)) = &broker {
        if !value.is_empty() {
            return Ok(value.clone());
        }
    }
    let broker = match broker {
        Ok(_) => format!("{APPLE_SIGNING_CERTIFICATE_ITEM} has no {field}"),
        Err(error) => error.to_string(),
    };
    crate::credential_store::owner::read_string(APPLE_SIGNING_CERTIFICATE_ITEM, field).map_err(
        |owner| {
            DeployError(format!(
                "cannot read {APPLE_SIGNING_CERTIFICATE_ITEM}#{field} for native signing: \
                 broker: {broker}; owner vault: {owner}"
            ))
        },
    )
}

/// Resolve the pinned shared signer this fleet signs native code with,
/// installing it into its Stado-owned cache when the host has none.
///
/// Deliberately not a PATH lookup: the signature a host produces has to come
/// from one reviewed signer revision, and a machine's own `wisent-products`
/// may be any older one.
async fn signing_program(
    target: &ComputeTarget,
    home: &str,
    runner: &Runner,
) -> Result<String, DeployError> {
    bootstrap_signer(target, home, runner).await
}

async fn bootstrap_signer(
    target: &ComputeTarget,
    home: &str,
    runner: &Runner,
) -> Result<String, DeployError> {
    use base64::Engine;

    // Private build input from wisent-products 319a6fb, never a public release.
    const SOURCE_SHA256: &str = "277271b7a548d0d09a0889d063c997baeef6de90787255249800705aac2ad484";
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
            include_str!("../../../host_payloads/native_signing/runtime.py"),
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
