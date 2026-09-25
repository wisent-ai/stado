//! Fleet signing credentials and the signer that consumes them: Stado's own
//! `stado product signing`. A builder signs with the Stado running its job; a
//! provisioned host signs with the Stado the fleet installed on it.

use crate::deploy::shlex_quote;

use crate::deploy::host_channel;
use crate::deploy::{DeployError, Runner};
use crate::targets::ComputeTarget;
/// The Skarbiec item holding the Apple certificate and key this fleet signs
/// native code with. A build host keeps no identity of its own.
const APPLE_SIGNING_CERTIFICATE_ITEM: &str = "desktop-signing-apple-development";
/// Apple's WWDR G3 intermediate, the issuer of that certificate. A Mac without
/// it builds no chain and reports the certificate as no identity at all.
pub const APPLE_ISSUER_CHAIN_SHA256: &str =
    "e9473d95d06080920600a0101bf47581906ea21810c67b71ad39616be3c55b4b";

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

/// The environment that hands the fleet's Apple identity to the signer on
/// this very machine: the certificate with Apple's issuer chain appended, and
/// the private key, as `stado product signing` reads them into a temporary
/// keychain it removes afterwards. A build host keeps no identity of its own,
/// so a darwin release signs the same way on every builder the fleet may place
/// it on.
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

/// The argv prefix that runs this machine's signer: the executable running
/// now, so a build job signs with exactly the Stado that runs it.
pub fn local_signer() -> Vec<String> {
    let executable = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "stado".into());
    vec![executable, "product".into()]
}

/// The Stado the fleet installed on `target`, after it has answered that it
/// carries `product signing`. A host whose Stado predates the command is
/// refused by name: converging its Stado release is the repair, and no other
/// signing program is looked for.
pub(crate) async fn host_signer(
    target: &ComputeTarget,
    home: &str,
    runner: &Runner,
) -> Result<String, DeployError> {
    let program = format!("{home}/.stado/bin/stado");
    let probe = host_channel::run_program(
        target,
        &[program.as_str(), "product", "signing", "sign", "--help"],
        runner,
    )
    .await?;
    if !probe.ok() {
        return Err(DeployError(format!(
            "{}: {program} cannot sign native code: `stado product signing sign` is not \
             available there ({}); converge Stado on this host first",
            target.name,
            probe.detail().trim()
        )));
    }
    Ok(program)
}

/// Run the compiled runner reconciliation with credentials confined to stdin
/// and the signing child's environment, never a persistent host keychain.
/// The script signs through `"$STADO_BIN" product signing`.
pub(crate) async fn run_runner_reconciliation(
    target: &ComputeTarget,
    script: &str,
    runner: &Runner,
) -> Result<crate::deploy::CommandOutput, DeployError> {
    let home = host_channel::remote_home(target, runner).await?;
    let signer = host_signer(target, &home, runner).await?;
    let mut prepared = format!("set -e\nexport STADO_BIN={}\n", shlex_quote(&signer));
    for (name, value) in signing_environment().await? {
        use std::fmt::Write;
        writeln!(&mut prepared, "export {name}={}", shlex_quote(&value))
            .map_err(|error| DeployError(format!("cannot prepare signing environment: {error}")))?;
    }
    prepared.push_str(script);
    host_channel::run_script(target, &prepared, runner).await
}
