//! The one operator value this surface takes, carried to Sunshine's own API on
//! the host rather than through a browser.

use std::net::{Ipv4Addr, SocketAddrV4};

use serde_json::Value;

use crate::deploy::stream::{parse_fields, report, CREDENTIAL_FILE};
use crate::deploy::{host_channel, DeployError, Runner};
use crate::stream::schema::SUNSHINE_HTTPS_PORT;
use crate::targets::ComputeTarget;

/// Hand Moonlight's PIN to Sunshine.
///
/// The PIN is the digits the client just generated and it authorises exactly
/// one pairing, so it is the one operator value this surface takes. It is
/// checked to be digits before it is substituted; whether it is the PIN
/// Moonlight shows is Sunshine's answer, which the report carries. The web
/// credentials never leave the host. The request carries no time cut of its
/// own: Sunshine's answer or curl's failure ends it.
pub async fn pair(
    target: &ComputeTarget,
    pin: &str,
    client_name: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    if pin.is_empty() || !pin.chars().all(|c| c.is_ascii_digit()) {
        return Err(DeployError(format!(
            "pin {pin:?} is not the digits Moonlight shows"
        ))
        .stating(crate::primitives::failure::FailureCode::Refused));
    }
    if !client_name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(DeployError(format!(
            "client name {client_name:?} must be letters, digits, dash or underscore"
        ))
        .stating(crate::primitives::failure::FailureCode::Refused));
    }
    let script = r#"set -euo pipefail
# The report carries stdout only, so a script whose error goes to stderr fails
# invisibly. Fold the two together: a host operation that breaks must say why.
exec 2>&1
[ -r CREDENTIAL_FILE ] || { printf 'ERROR\tno web credentials at CREDENTIAL_FILE; run `stado stream apply` first\n' >&2; exit 1; }
user=$(cut -d: -f1 CREDENTIAL_FILE)
secret=$(cut -d: -f2- CREDENTIAL_FILE)
response=$(mktemp)
trap 'rm -f "$response"' EXIT
status=$(curl -sS -k -o "$response" -w '%{http_code}' \
  -u "$user:$secret" \
  -H 'Content-Type: application/json' \
  -X POST "https://SUNSHINE_ADDRESS/api/pin" \
  --data '{"pin":"PIN","name":"CLIENT_NAME"}')
printf 'HTTP\t%s\n' "$status"
printf 'BODY\t%s\n' "$(tr -d '\n' <"$response")"
case "$status" in 200) printf 'PAIRED\tCLIENT_NAME\n' ;; *) exit 1 ;; esac
"#
    .replace("CREDENTIAL_FILE", CREDENTIAL_FILE)
    .replace(
        "SUNSHINE_ADDRESS",
        &SocketAddrV4::new(Ipv4Addr::LOCALHOST, SUNSHINE_HTTPS_PORT).to_string(),
    )
    .replace("CLIENT_NAME", client_name)
    .replace("PIN", pin);
    let output = host_channel::run_script(target, &script, runner).await?;
    let mut body = report(target, &output, "paired");
    if let Some(map) = body.as_object_mut() {
        map.insert(
            "fields".to_string(),
            Value::Object(parse_fields(&output.stdout)),
        );
    }
    Ok(body)
}
