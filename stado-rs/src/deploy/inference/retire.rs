//! Teardown of the unit, the container, the reservation and the optional cache.

use serde_json::Value;

use super::support::{report, safe_runtime, unit_name};
use crate::deploy::{host_channel, shlex_quote, DeployError, Runner};
use crate::inference::schema::Deployment;
use crate::targets::ComputeTarget;

pub async fn retire(
    target: &ComputeTarget,
    deployment: &Deployment,
    purge_cache: bool,
    runner: &Runner,
) -> Result<Value, DeployError> {
    safe_runtime(deployment)?;
    let unit = shlex_quote(&unit_name(&deployment.name));
    let name = shlex_quote(&deployment.name);
    let image = shlex_quote(&deployment.engine.image);
    let cache_dir = deployment
        .resources
        .cache_dir
        .as_deref()
        .map(shlex_quote)
        .unwrap_or_else(|| "\"$root/cache\"".to_string());
    let purge = if purge_cache {
        format!(
            "docker run --rm --entrypoint /bin/sh -v \"$cache_dir:/stado-cache\" {image} -c 'rm -rf /stado-cache/* /stado-cache/.[!.]* /stado-cache/..?*'; rmdir \"$cache_dir\" 2>/dev/null || true"
        )
    } else {
        ":".to_string()
    };
    let script = format!(
        r#"set -euo pipefail
unit={unit}
name={name}
root="$HOME/.stado/inference/$name"
cache_dir={cache_dir}
systemctl --user disable --now "$unit" || true
docker rm -f "stado-inference-$name" >/dev/null 2>&1 || true
rm -f "$HOME/.config/systemd/user/$unit"
if [ -f "$HOME/.stado/inference/reservation.json" ] && grep -F '"deployment":"'"$name"'"' "$HOME/.stado/inference/reservation.json" >/dev/null; then
  rm -f "$HOME/.stado/inference/reservation.json"
fi
rm -f "$root/api-key" "$root/runtime.env"
{purge}
systemctl --user daemon-reload
printf 'STATUS\tretired\n'
"#
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    Ok(report(target, &output, "retired"))
}
