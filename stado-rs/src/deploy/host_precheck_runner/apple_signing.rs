//! The Apple Developer ID material: the capability the trajectory redeems, the
//! programs that make and finish the certificate, and the secrets it publishes.

use serde_json::{json, Value};

use super::github::{repository_name, set_repository_secret};
use crate::deploy::{host_capability, host_channel, production_runner, DeployError, Runner};
use crate::targets::ComputeTarget;

pub(crate) const DEVELOPER_ID_ITEM: &str = "desktop-release-developer-id";
const MACOS_CERT_P12_SECRET: &str = "MACOS_CERT_P12";
const MACOS_CERT_PASSWORD_SECRET: &str = "MACOS_CERT_PASSWORD";
const MACOS_SIGN_IDENTITY_SECRET: &str = "MACOS_SIGN_IDENTITY";
pub(crate) const APPLE_DEVELOPER_ID_ACTION: &str = "apple_create_developer_id";

pub(crate) fn publish_developer_id_secrets(
    repositories: &[String],
    p12: &str,
    password: &str,
    identity: &str,
    github_token: &str,
) -> Result<(), DeployError> {
    for repository in repositories {
        let repository = repository_name(repository)?;
        for (name, value) in [
            (MACOS_CERT_P12_SECRET, p12),
            (MACOS_CERT_PASSWORD_SECRET, password),
            (MACOS_SIGN_IDENTITY_SECRET, identity),
        ] {
            set_repository_secret(repository, name, value, github_token)?;
        }
    }
    Ok(())
}

pub(crate) fn developer_id_bundle() -> Result<Option<(String, String, String, String)>, DeployError>
{
    if !crate::credential_store::owner::item_exists(DEVELOPER_ID_ITEM)
        .map_err(|error| DeployError(error.to_string()))?
    {
        return Ok(None);
    }
    let read = |field| {
        crate::credential_store::owner::read_string(DEVELOPER_ID_ITEM, field)
            .map_err(|error| DeployError(error.to_string()))
    };
    Ok(Some((
        read("p12")?,
        read("password")?,
        read("identity")?,
        read("not_after")?,
    )))
}
/// Issue one Apple sign-in capability on the host that will redeem it.
///
/// This used to shell out to a local `skarbiec capability-issue`, and
/// [`crate::deploy::host_capability`] was written on 2026-08-31 for exactly
/// that gap while naming this function as the surviving instance of it.
/// Capabilities are per-host at both ends: issuing writes state beside the
/// issuing machine's vault, and redemption is a UNIX socket on the worker. So a
/// reference minted on an operator's laptop named nothing on
/// charless-mac-mini, the broker answered `redemption denied: no such
/// capability`, and the trajectory reported `capability denied` after
/// spending its browser session - which is how a Developer ID run could look
/// authorized and be unredeemable at the same time.
///
/// The broker addressed is Weles's own instance, not the vault's default
/// pair: its launcher serves `weles-api-capability.sock` out of
/// `weles-api-capabilities.json`, and anything issued into the default files
/// is invisible to it.
pub(crate) async fn issue_apple_capability(
    target: &ComputeTarget,
    broker: &host_capability::RemoteBroker,
    agent: &str,
    purpose: &str,
    resource: &str,
    authorization_id: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let capability_id = host_capability::issue(
        target,
        broker,
        &host_capability::Issuance {
            agent,
            purpose,
            resource,
            capability_target: "weles",
            ttl_seconds: "3600",
            max_uses: "1",
            // Bound on purpose: the Apple trajectory builds its expectation
            // with the guard id, so an unbound reference is refused with
            // `capability operation mismatch`.
            authorization_id: Some(authorization_id),
        },
        runner,
    )
    .await?;
    if capability_id.len() != CAPABILITY_ID_HEX_DIGITS
        || !capability_id.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(DeployError(format!(
            "{}: skarbiec issued {capability_id:?} for {resource}, which is not a capability id",
            target.name
        )));
    }
    Ok(json!({
        "capability_id": capability_id,
        "purpose": purpose,
        "resource": resource,
        "target": "weles",
        "authorization_id": authorization_id,
    }))
}

/// A Skarbiec capability id is a 256-bit value printed as hex, so anything of
/// another length is a message rather than a reference.
const CAPABILITY_ID_HEX_DIGITS: usize = 64;

pub(crate) async fn required_remote_file(
    target: &ComputeTarget,
    path: &str,
) -> Result<String, DeployError> {
    host_channel::remote_read_file(target, path, &production_runner())
        .await?
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| DeployError(format!("{} did not produce {path}", target.name)))
}

pub(crate) const DEVELOPER_ID_PREPARE: &str = r#"set -euo pipefail
umask 077
work=__WORK_DIR__
mkdir -p "$work"
if [ ! -s "$work/private-key.pem" ]; then
  /usr/bin/openssl genrsa -out "$work/private-key.pem" 3072
fi
if [ ! -s "$work/request.csr" ]; then
  /usr/bin/openssl req -new -sha256 -key "$work/private-key.pem" -out "$work/request.csr" -subj "/CN=Wisent Desktop Release"
fi
/usr/bin/openssl req -in "$work/request.csr" -noout -verify
"#;

pub(crate) const DEVELOPER_ID_FINISH: &str = r#"set -euo pipefail
umask 077
work=__WORK_DIR__
[ -s "$work/private-key.pem" ]
[ -s "$work/certificate.cer" ]
if /usr/bin/openssl x509 -in "$work/certificate.cer" -noout >/dev/null 2>&1; then
  /bin/cp "$work/certificate.cer" "$work/certificate.pem"
else
  /usr/bin/openssl x509 -inform DER -in "$work/certificate.cer" -out "$work/certificate.pem"
fi
identity=$(/usr/bin/openssl x509 -in "$work/certificate.pem" -noout -subject -nameopt RFC2253 | /usr/bin/sed -n 's/^subject=.*CN=\([^,]*\).*$/\1/p')
[ -n "$identity" ]
printf '%s\n' "$identity" | /usr/bin/grep -F 'Developer ID Application:' >/dev/null
password=$(/usr/bin/openssl rand -hex 24)
/usr/bin/openssl pkcs12 -export -inkey "$work/private-key.pem" -in "$work/certificate.pem" -out "$work/certificate.p12" -passout "pass:$password" -name "$identity"
/usr/bin/base64 < "$work/certificate.p12" | /usr/bin/tr -d '\n' > "$work/certificate.p12.b64"
printf '%s\n' "$password" > "$work/certificate.password"
printf '%s\n' "$identity" > "$work/certificate.identity"
/usr/bin/openssl x509 -in "$work/certificate.pem" -noout -enddate | /usr/bin/sed 's/^notAfter=//' > "$work/certificate.not-after"
"#;
