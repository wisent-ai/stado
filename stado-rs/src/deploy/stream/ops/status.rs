//! What a session already installed is doing, asked of the host and nothing
//! else.

use serde_json::Value;

use crate::deploy::stream::{library_dir, parse_fields, report, SUNSHINE_UNIT, XORG_UNIT};
use crate::deploy::{host_channel, DeployError, Runner};
use crate::stream::schema::{DISPLAY, SUNSHINE_HTTPS_PORT};
use crate::targets::ComputeTarget;

/// What the session is doing right now: units, the screen's real size, the
/// board carrying it, ports, paired clients, and room left for the library.
pub async fn status(target: &ComputeTarget, runner: &Runner) -> Result<Value, DeployError> {
    let script = r#"set -euo pipefail
# The report carries stdout only, so a script whose error goes to stderr fails
# invisibly. Fold the two together: a host operation that breaks must say why.
exec 2>&1
printf 'XORG\t%s\n' "$(systemctl is-active XORG_UNIT 2>&1 || true)"
printf 'SUNSHINE\t%s\n' "$(systemctl is-active SUNSHINE_UNIT 2>&1 || true)"
printf 'SESSION\t'
if DISPLAY=DISPLAY_NUMBER xdpyinfo >/dev/null 2>&1; then
  # No early `exit` in awk: it closes the pipe, xdpyinfo takes SIGPIPE, and
  # pipefail turns a healthy screen into a failed script (exit 141).
  DISPLAY=DISPLAY_NUMBER xdpyinfo | awk '/dimensions:/ { print $2 }' | sed -n 1p
else
  printf 'no display on DISPLAY_NUMBER\n'
fi
printf 'RENDERING_BOARD\t'
rendering=$(nvidia-smi --query-compute-apps=gpu_uuid,process_name --format=csv,noheader 2>/dev/null | grep -i -E 'Xorg|sunshine' | sed -n 1p || true)
printf '%s\n' "${rendering:-idle (nothing rendering yet)}"
printf 'PORTS\t'
ss -ltn 2>/dev/null | awk '$4 ~ /:479[89][0-9]$/ { printf "%s ", $4 }' || true
printf '\n'
# The state file travels back whole, base64 on one line; Stado counts the
# paired devices itself, so the host needs no interpreter to read JSON.
printf 'PAIRED_CLIENTS_STATE\t'
state=/root/.config/sunshine/sunshine_state.json
if [ -r "$state" ]; then
  base64 -w0 "$state" 2>/dev/null || printf 'unreadable'
  printf '\n'
else
  printf 'absent\n'
fi
printf 'LIBRARY\t'
df -Ph "LIBRARY_DIR" 2>/dev/null | awk 'NR==2 { print $1, $4 " available" }' || printf 'absent\n'
printf 'XORG_LOG\t%s\n' "$(journalctl -u XORG_UNIT --no-pager -n 3 -o cat 2>/dev/null | tr '\n' '|' || true)"
printf 'SUNSHINE_LOG\t%s\n' "$(journalctl -u SUNSHINE_UNIT --no-pager -n 3 -o cat 2>/dev/null | tr '\n' '|' || true)"
printf 'CLIENT_ENDPOINT\t'
if command -v tailscale >/dev/null; then
  tailscale ip 2>/dev/null | while IFS= read -r address; do case "$address" in *:*) ;; *) printf '%s\n' "$address"; break ;; esac; done
else
  printf 'unknown\n'
fi
"#
    .replace("XORG_UNIT", XORG_UNIT)
    .replace("SUNSHINE_UNIT", SUNSHINE_UNIT)
    .replace("LIBRARY_DIR", &library_dir(target))
    .replace("DISPLAY_NUMBER", DISPLAY);
    let output = host_channel::run_script(target, &script, runner).await?;
    let mut body = report(target, &output, "reported");
    let mut fields = parse_fields(&output.stdout);
    let state = fields.remove("paired_clients_state");
    fields.insert("paired_clients".to_string(), paired_clients(state.as_ref()));
    if let Some(map) = body.as_object_mut() {
        map.insert("fields".to_string(), Value::Object(fields));
        map.insert("client_port".to_string(), Value::from(SUNSHINE_HTTPS_PORT));
    }
    Ok(body)
}

/// How many devices Sunshine has paired, from its state file as the host
/// sent it: `0` without a file, `unreadable` when it cannot be decoded.
fn paired_clients(state: Option<&Value>) -> Value {
    use base64::Engine as _;
    let Some(encoded) = state.and_then(Value::as_str) else {
        return Value::from("unreadable");
    };
    if encoded == "absent" {
        return Value::from("0");
    }
    base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .map(|document| {
            let root = document.get("root").unwrap_or(&document);
            let count = root["devices"].as_array().map_or(0, Vec::len);
            Value::from(count.to_string())
        })
        .unwrap_or_else(|| Value::from("unreadable"))
}
