//! The one operator value this surface takes, carried to Sunshine's own API on
//! the host rather than through a browser.

use serde_json::Value;

use crate::deploy::stream::{parse_fields, report, CREDENTIAL_FILE};
use crate::deploy::{host_channel, DeployError, Runner};
use crate::stream::schema::SUNSHINE_HTTPS_PORT;
use crate::targets::ComputeTarget;

/// Hand Moonlight's PIN to Sunshine.
///
/// The PIN is four digits the client just generated and it authorises exactly
/// one pairing, so it is the one operator value this surface takes. It is
/// checked for shape before it is substituted, and the web credentials never
/// leave the host.
pub async fn pair(
    target: &ComputeTarget,
    pin: &str,
    client_name: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    if pin.len() != "0000".len() || !pin.chars().all(|c| c.is_ascii_digit()) {
        return Err(DeployError(format!(
            "pin {pin:?} is not the four digits Moonlight shows"
        )));
    }
    if !client_name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(DeployError(format!(
            "client name {client_name:?} must be letters, digits, dash or underscore"
        )));
    }
    let script = r#"set -euo pipefail
# The report carries stdout only, so a script whose error goes to stderr fails
# invisibly. Fold the two together: a host operation that breaks must say why.
exec 2>&1
[ -r CREDENTIAL_FILE ] || { printf 'ERROR\tno web credentials at CREDENTIAL_FILE; run `stado stream apply` first\n' >&2; exit 1; }
user=$(cut -d: -f1 CREDENTIAL_FILE)
secret=$(cut -d: -f2- CREDENTIAL_FILE)
status=$(curl -sS -k -o /tmp/stado-stream-pair.$$ -w '%{http_code}' \
  --max-time 20 \
  -u "$user:$secret" \
  -H 'Content-Type: application/json' \
  -X POST "https://127.0.0.1:SUNSHINE_HTTPS_PORT/api/pin" \
  --data '{"pin":"PIN","name":"CLIENT_NAME"}')
printf 'HTTP\t%s\n' "$status"
printf 'BODY\t%s\n' "$(tr -d '\n' </tmp/stado-stream-pair.$$ | cut -c1-200)"
rm -f /tmp/stado-stream-pair.$$
case "$status" in 200) printf 'PAIRED\tCLIENT_NAME\n' ;; *) exit 1 ;; esac
"#
    .replace("CREDENTIAL_FILE", CREDENTIAL_FILE)
    .replace("SUNSHINE_HTTPS_PORT", &SUNSHINE_HTTPS_PORT.to_string())
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
