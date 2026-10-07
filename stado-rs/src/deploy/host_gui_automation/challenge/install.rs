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
        return Err(DeployError::unreachable(format!(
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
        .ok_or_else(|| {
            DeployError("Apple challenge helper was not installed".to_string())
                .stating(crate::primitives::failure::FailureCode::NotFound)
        })?;
    if identity.version != APPLE_CHALLENGE_HELPER_VERSION {
        return Err(DeployError::unreachable(format!(
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

/// Run on the build host with `$1` the host's Stado, `$2` the bundle
/// identifier, `$3` the staged file and `$4` the previous signature (may be
/// empty); stdin carries the certificate chain and the private key, one base64
/// line each, which reach the signer only through its environment.
const NATIVE_SIGN: &str = r#"set -eu
IFS= read -r certificate || exit
IFS= read -r key || exit
WISENT_CODESIGN_CERTIFICATE_PEM=$(printf '%s' "$certificate" | /usr/bin/base64 -d)
WISENT_CODESIGN_PRIVATE_KEY_PEM=$(printf '%s' "$key" | /usr/bin/base64 -d)
export WISENT_CODESIGN_CERTIFICATE_PEM WISENT_CODESIGN_PRIVATE_KEY_PEM
if [ -n "$4" ]; then
  exec "$1" product signing sign --identifier "$2" --previous "$4" "$3" --json
fi
exec "$1" product signing sign --identifier "$2" "$3" --json
"#;

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
    use crate::deploy::native_signing::{signing_credential, APPLE_ISSUER_CHAIN_SHA256};
    // Apple's intermediate is not on every Mac, and its absence makes the
    // certificate unusable without saying so, so the issuer travels with it.
    let issuers = String::from_utf8(
        crate::deploy::native_signing::pinned_artifact(
            &format!("apple-issuers-{APPLE_ISSUER_CHAIN_SHA256}.pem"),
            APPLE_ISSUER_CHAIN_SHA256,
        )
        .await?,
    )
    .map_err(|error| {
        DeployError(format!("Apple issuer chain is not text: {error}"))
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    use base64::Engine as _;
    let encode = |text: &str| base64::engine::general_purpose::STANDARD.encode(text);
    let certificate = signing_credential("certificate").await?;
    let certificate = format!("{}\n{issuers}", certificate.trim_end());
    let private_key = signing_credential("private_key").await?;
    // The certificate and its key travel on stdin, one base64 line each, and
    // reach `stado product signing sign` only through its environment, never
    // its argv, so a process listing on this host cannot read either.
    let stdin = format!("{}\n{}\n", encode(&certificate), encode(&private_key));
    let output = host_channel::run_program_with_stdin(
        target,
        &[
            "/bin/sh",
            "-c",
            NATIVE_SIGN,
            "stado-native-sign",
            signer,
            APPLE_CHALLENGE_HELPER_BUNDLE_ID,
            staged,
            previous,
        ],
        &stdin,
        runner,
    )
    .await?;
    if !output.ok() {
        return Err(DeployError::unreachable(format!(
            "{}: sign Apple challenge helper failed: {}",
            target.name,
            output.detail().trim()
        )));
    }
    // One target in, one report out: the CLI answers with a list either way.
    let reports: serde_json::Value = serde_json::from_str(&output.stdout).map_err(|error| {
        DeployError(format!("invalid Apple challenge signing receipt: {error}"))
    })?;
    let report = &reports[0];
    if report["state"].as_str() != Some("stable") {
        return Err(DeployError::unreachable(format!(
            "{}: Apple challenge helper signature is {}",
            target.name, report["state"]
        )));
    }
    Ok(())
}

/// The host's own Stado, which signs through `stado product signing`. The
/// probe and its refusal live in [`crate::deploy::native_signing`], shared
/// with runner reconciliation.
async fn signing_program(
    target: &ComputeTarget,
    home: &str,
    runner: &Runner,
) -> Result<String, DeployError> {
    crate::deploy::native_signing::host_signer(target, home, runner).await
}
