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
    // Apple's intermediate is not on every Mac, and its absence makes the
    // certificate unusable without saying so, so the issuer travels with it.
    let issuers = String::from_utf8(
        crate::deploy::native_signing::pinned_artifact(
            &format!("apple-issuers-{APPLE_ISSUER_CHAIN_SHA256}.pem"),
            APPLE_ISSUER_CHAIN_SHA256,
        )
        .await?,
    )
    .map_err(|error| DeployError(format!("Apple issuer chain is not text: {error}")))?;
    let certificate = signing_credential("certificate").await?;
    let request = serde_json::json!({
        "program": signer,
        "identifier": APPLE_CHALLENGE_HELPER_BUNDLE_ID,
        "target": staged,
        "previous": previous,
        "certificate": format!("{}\n{issuers}", certificate.trim_end()),
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
/// installing it into its Stado-owned cache when the host has none. The
/// pin, the payload and the receipt check live in
/// [`crate::deploy::native_signing`], shared with the release worker.
async fn signing_program(
    target: &ComputeTarget,
    home: &str,
    runner: &Runner,
) -> Result<String, DeployError> {
    crate::deploy::native_signing::bootstrap_remote_signer(target, home, runner).await
}
