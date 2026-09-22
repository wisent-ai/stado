//! Fleet signing credentials and the qualified native Rust SDK that consumes
//! them. Builders and provisioned hosts use the same immutable SDK release;
//! an existing executable alone is never accepted as provenance.

pub mod runtime;

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
    let signer = runtime::on_host(target, &home, runner).await?;
    let mut prepared = format!(
        "set -e\nexport WISENT_PRODUCTS_BIN={}\n",
        crate::deploy::shlex_quote(&signer)
    );
    for (name, value) in signing_environment().await? {
        use std::fmt::Write;
        writeln!(
            &mut prepared,
            "export {name}={}",
            crate::deploy::shlex_quote(&value)
        )
        .map_err(|error| DeployError(format!("cannot prepare signing environment: {error}")))?;
    }
    prepared.push_str(script);
    host_channel::run_script(target, &prepared, runner).await
}
