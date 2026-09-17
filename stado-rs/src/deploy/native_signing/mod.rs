//! The one native code signer this fleet signs with, and the two places it
//! has to be present: on a host Stado provisions over its channel, and on the
//! builder that runs `stado release worker` for a darwin platform.
//!
//! Deliberately not a PATH lookup, in either place. The signature a host or a
//! build produces has to come from one reviewed signer revision, and a
//! machine's own `wisent-products` may be any older one - or none: on
//! 2026-09-10 the release worker on charless-mac-mini looked the signer up on
//! PATH, found nothing, and weles-worker 0.6.6 died at `macos-code-signing`
//! with "cannot run wisent-products: No such file or directory", while the
//! same host had been provisioned with the pinned signer for GUI automation
//! the day before. The source is one private build input, addressed by its own
//! digest in the fleet's object namespace, installed into a Stado-owned cache
//! by the same payload in both directions.

use base64::Engine;

use crate::deploy::host_channel;
use crate::deploy::{CommandSpec, DeployError, Runner};
use crate::targets::ComputeTarget;
/// The Skarbiec item holding the Apple certificate and key this fleet signs
/// native code with. A build host keeps no identity of its own.
const APPLE_SIGNING_CERTIFICATE_ITEM: &str = "desktop-signing-apple-development";
/// Apple's WWDR G3 intermediate, the issuer of that certificate. A Mac without
/// it builds no chain and reports the certificate as no identity at all.
pub(crate) const APPLE_ISSUER_CHAIN_SHA256: &str =
    "e9473d95d06080920600a0101bf47581906ea21810c67b71ad39616be3c55b4b";

/// Private build input from wisent-products 7aa6f1f, never a public release.
pub const SIGNER_SOURCE_SHA256: &str =
    "6a2781e2a70a1fa7160ac5562332f29954e2c81f799ff50a69f12eef94c9bd24";

const RUNTIME_PAYLOAD: &str = include_str!("../../host_payloads/native_signing/runtime.py");

/// Where the pinned signer lives once installed, under the given home.
pub fn signer_program(home: &str) -> String {
    format!("{home}/.stado/cache/native-signing/{SIGNER_SOURCE_SHA256}/bin/wisent-products")
}

/// The payload's request: the source bytes and the digest they must carry.
async fn runtime_request() -> Result<String, DeployError> {
    let source = pinned_artifact(
        &format!("{SIGNER_SOURCE_SHA256}.tar.gz"),
        SIGNER_SOURCE_SHA256,
    )
    .await?;
    Ok(serde_json::json!({
        "archive": base64::engine::general_purpose::STANDARD.encode(source),
        "sha256": SIGNER_SOURCE_SHA256,
    })
    .to_string())
}

/// The payload's receipt must name the program this fleet pinned, or the
/// installation is not the one every signature is attributed to.
fn checked_receipt(stdout: &str, program: &str) -> Result<String, DeployError> {
    let observed: serde_json::Value = serde_json::from_str(stdout)
        .map_err(|error| DeployError(format!("invalid native signing runtime receipt: {error}")))?;
    if observed["program"].as_str() != Some(program)
        || observed["source_sha256"].as_str() != Some(SIGNER_SOURCE_SHA256)
    {
        return Err(DeployError(
            "native signing runtime returned another source or program".into(),
        ));
    }
    Ok(program.to_string())
}

/// Resolve the pinned signer on a host reached over the Stado channel,
/// installing it into the host's Stado-owned cache when the host has none.
pub async fn bootstrap_remote_signer(
    target: &ComputeTarget,
    home: &str,
    runner: &Runner,
) -> Result<String, DeployError> {
    let program = signer_program(home);
    if host_channel::remote_test(
        target,
        &format!("-x {}", crate::deploy::shlex_quote(&program)),
        runner,
    )
    .await?
    {
        return Ok(program);
    }
    let input = runtime_request().await?;
    let output = host_channel::run_program_with_stdin(
        target,
        &["/usr/bin/python3", "-c", RUNTIME_PAYLOAD],
        &input,
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
    checked_receipt(&output.stdout, &program)
}

/// Resolve the pinned signer on this very machine - the builder a darwin
/// release job runs on - installing it the same way when it is absent.
pub async fn bootstrap_local_signer(runner: &Runner) -> Result<String, DeployError> {
    let home = std::env::var("HOME").map_err(|_| {
        DeployError("HOME is unset, so the native signing cache has no root".into())
    })?;
    let program = signer_program(&home);
    if std::fs::metadata(&program).is_ok_and(|metadata| metadata.is_file()) {
        return Ok(program);
    }
    let input = runtime_request().await?;
    let output = runner(CommandSpec {
        argv: vec![
            "/usr/bin/python3".into(),
            "-c".into(),
            RUNTIME_PAYLOAD.into(),
        ],
        stdin: Some(input),
        timeout: None,
    })
    .await
    .map_err(DeployError)?;
    if !output.ok() {
        return Err(DeployError(format!(
            "native signing runtime preparation failed on this builder: {}",
            output.detail().trim()
        )));
    }
    checked_receipt(&output.stdout, &program)
}

/// One immutable native-signing input, addressed by its own digest in the
/// fleet's object namespace and verified before anything uses it.
pub async fn pinned_artifact(leaf: &str, sha256: &str) -> Result<Vec<u8>, DeployError> {
    let namespace = crate::config::wc_stado_storage_namespace();
    if namespace.is_empty() {
        return Err(DeployError(
            "storage.stado.namespace is not configured, so no native signing input can be read"
                .into(),
        ));
    }
    let uri = format!("stado://{namespace}/artifacts/native-signing/{leaf}");
    let bytes = crate::cli::storage::fetch_object(&uri)
        .await
        .map_err(|error| DeployError(format!("cannot read native signing input {uri}: {error}")))?;
    if crate::release_control::sha256_bytes(&bytes) != sha256 {
        return Err(DeployError(format!(
            "native signing input digest mismatch: {uri}"
        )));
    }
    Ok(bytes)
}
/// One field of the fleet's Apple signing certificate: the broker grant first,
/// then the owner vault, naming both failures rather than one.
pub(crate) async fn signing_credential(field: &str) -> Result<String, DeployError> {
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

/// The environment that hands the fleet's Apple identity to the pinned
/// signer on this very machine: the certificate with Apple's issuer chain
/// appended, and the private key, as `wisent-products signing` reads them
/// into a temporary keychain it removes afterwards. A build host keeps no
/// identity of its own, so a darwin release signs the same way on every
/// builder the fleet may place it on.
pub async fn signing_environment() -> Result<Vec<(String, String)>, DeployError> {
    let issuers = String::from_utf8(
        pinned_artifact(
            &format!("apple-issuers-{APPLE_ISSUER_CHAIN_SHA256}.pem"),
            APPLE_ISSUER_CHAIN_SHA256,
        )
        .await?,
    )
    .map_err(|error| DeployError(format!("Apple issuer chain is not text: {error}")))?;
    let certificate = signing_credential("certificate").await?;
    let private_key = signing_credential("private_key").await?;
    Ok(vec![
        (
            "WISENT_CODESIGN_CERTIFICATE_PEM".into(),
            format!("{}\n{issuers}", certificate.trim_end()),
        ),
        ("WISENT_CODESIGN_PRIVATE_KEY_PEM".into(), private_key),
    ])
}

/// Run the compiled runner reconciliation with credentials confined to stdin
/// and the signing child's environment, never a persistent host keychain.
pub(crate) async fn run_runner_reconciliation(
    target: &ComputeTarget,
    script: &str,
    runner: &Runner,
) -> Result<crate::deploy::CommandOutput, DeployError> {
    let home = host_channel::remote_home(target, runner).await?;
    let signer = bootstrap_remote_signer(target, &home, runner).await?;
    let issuers = String::from_utf8(
        pinned_artifact(
            &format!("apple-issuers-{APPLE_ISSUER_CHAIN_SHA256}.pem"),
            APPLE_ISSUER_CHAIN_SHA256,
        )
        .await?,
    )
    .map_err(|error| DeployError(format!("Apple issuer chain is not text: {error}")))?;
    let certificate = signing_credential("certificate").await?;
    let request = serde_json::json!({
        "signer": signer,
        "apphost_signer": include_str!("../../host_payloads/native_signing/runner_apphosts.py"),
        "script": script,
        "certificate": format!("{}\n{issuers}", certificate.trim_end()),
        "private_key": signing_credential("private_key").await?,
    });
    host_channel::run_program_with_stdin(
        target,
        &[
            "/usr/bin/python3",
            "-c",
            include_str!("../../host_payloads/native_signing/runner.py"),
        ],
        &request.to_string(),
        runner,
    )
    .await
}
