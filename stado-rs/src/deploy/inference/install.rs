//! Container start and the single-writer reservation record it publishes.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::Value;

use super::support::{report, safe_runtime, startup_timeout, unit_name};
use crate::deploy::{host_channel, shlex_quote, DeployError, Runner};
use crate::inference::{reservation::Reservation, schema::Deployment};
use crate::targets::ComputeTarget;

pub async fn install(
    target: &ComputeTarget,
    deployment: &Deployment,
    api_key: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    safe_runtime(deployment)?;
    if api_key.is_empty() || api_key.chars().any(char::is_control) {
        return Err(DeployError(
            "inference bearer must be non-empty and single-line".to_string(),
        ));
    }
    let name = shlex_quote(&deployment.name);
    let image = shlex_quote(&deployment.engine.image);
    let endpoint_host = shlex_quote(&deployment.endpoint.host);
    let port = deployment.endpoint.port;
    let max_model_len = deployment.resources.max_model_len;
    let kv_cache_argument = deployment
        .resources
        .kv_cache_memory_gb
        .map(|gib| format!(" --kv-cache-memory {}", gib * 1024 * 1024 * 1024))
        .unwrap_or_default();
    let cache_dir = deployment
        .resources
        .cache_dir
        .as_deref()
        .map(shlex_quote)
        .unwrap_or_else(|| "\"$root/cache\"".to_string());
    let cache_mount = deployment
        .resources
        .cache_dir
        .as_deref()
        .map(|path| format!("{path}:/data/huggingface"))
        .unwrap_or_else(|| "\"$root/cache:/data/huggingface\"".to_string());
    let secret = shlex_quote(&STANDARD.encode(api_key));
    let unit = shlex_quote(&unit_name(&deployment.name));
    let reservation = Reservation {
        deployment: deployment.name.clone(),
        target: deployment.target.clone(),
        gpu_mode: deployment.resources.gpu_mode.clone(),
        engine: deployment.engine.name.clone(),
        model: deployment.model.repository.clone(),
        revision: deployment.model.revision.clone(),
        endpoint_host: deployment.endpoint.host.clone(),
        port: deployment.endpoint.port,
    };
    let reservation =
        shlex_quote(&STANDARD.encode(
            serde_json::to_vec(&reservation).map_err(|error| DeployError(error.to_string()))?,
        ));
    // A compute process is a blocker when nothing in the registry accounts for
    // it, not merely because it exists. The RTX host runs a declared game
    // stream unit whose encoder holds a few hundred MiB of a 96 GiB board, and
    // the blanket refusal made every deployment on that host impossible while
    // reporting the state as "unmanaged" — which the registry contradicts.
    // Anything the registry has never declared still refuses, so an unknown
    // training run keeps its GPU.
    let accounted = crate::deploy::service::declared_services(target)
        .into_iter()
        .flat_map(|declared| [declared.unit, declared.label, declared.name])
        .filter(|unit| !unit.trim().is_empty())
        .collect::<std::collections::BTreeSet<String>>()
        .into_iter()
        .collect::<Vec<String>>()
        .join("\n");
    let accounted = shlex_quote(&STANDARD.encode(accounted));
    let script = format!(
        r#"set -euo pipefail
name={name}
endpoint_host={endpoint_host}
unit={unit}
root="$HOME/.stado/inference/$name"
cache_dir={cache_dir}
reservation="$HOME/.stado/inference/reservation.json"
mkdir -p "$root" "$cache_dir" "$HOME/.config/systemd/user"
chmod u=rwx,go= "$HOME/.stado/inference" "$root" "$cache_dir"
if [ -f "$reservation" ] && ! grep -F '"deployment":"'"$name"'"' "$reservation" >/dev/null; then
  printf 'ERROR\tanother inference reservation exists\n'; exit 1
fi
printf '%s' {accounted} | base64 --decode > "$root/accounted-units"
chmod 600 "$root/accounted-units"
own_pids=$(docker top "stado-inference-$name" -eo pid 2>/dev/null | tr -dc '0-9\n' || true)
unaccounted=""
for pid in $(nvidia-smi --query-compute-apps=pid --format=csv,noheader,nounits | tr -dc '0-9\n'); do
  if printf '%s\n' "$own_pids" | grep -Fxq "$pid"; then continue; fi
  cgroup=$(cat "/proc/$pid/cgroup" 2>/dev/null || true)
  if [ -z "$cgroup" ]; then unaccounted="$unaccounted $pid"; continue; fi
  matched=""
  while IFS= read -r accounted_unit; do
    [ -n "$accounted_unit" ] || continue
    case "$cgroup" in *"$accounted_unit"*) matched=yes; break ;; esac
  done < "$root/accounted-units"
  [ -n "$matched" ] || unaccounted="$unaccounted $pid"
done
if [ -n "$unaccounted" ]; then
  printf 'ERROR\tGPU has an active compute process no registry unit accounts for:%s\n' "$unaccounted"
  exit 1
fi
if ss -ltn | grep -F "$endpoint_host:{port} " >/dev/null; then
  if ! docker ps --filter "name=^stado-inference-$name$" --format '{{{{.Names}}}}' | grep -Fx "stado-inference-$name" >/dev/null; then
    printf 'ERROR\tinference endpoint {port} is already in use\n'; false
  fi
fi
printf '%s' {secret} | base64 --decode > "$root/api-key"
printf '\n' >> "$root/api-key"
chmod 600 "$root/api-key"
printf 'VLLM_API_KEY=' > "$root/runtime.env"
cat "$root/api-key" >> "$root/runtime.env"
printf 'HF_HOME=/data/huggingface\n' >> "$root/runtime.env"
chmod 600 "$root/runtime.env"
printf '%s' {reservation} | base64 --decode > "$reservation"
chmod 600 "$reservation"
docker pull {image}
systemctl --user disable --now "$unit" || true
rm -f "$HOME/.config/systemd/user/$unit"
systemctl --user daemon-reload || true
docker rm -f "stado-inference-$name" || true
container=$(docker run --detach --restart unless-stopped --name "stado-inference-$name" --gpus all --network host --ipc host --env-file "$root/runtime.env" -v {cache_mount} {raw_image} --model {raw_repository} --revision {raw_revision} --served-model-name {raw_name} --host {raw_endpoint_host} --port {port} --max-model-len {max_model_len}{kv_cache_argument} --enable-auto-tool-choice --tool-call-parser hermes)
printf 'CONTAINER\t%s\n' "$container"
printf 'STATUS\tstarted\n'
"#,
        raw_name = deployment.name,
        raw_image = deployment.engine.image,
        raw_repository = deployment.model.repository,
        raw_revision = deployment.model.revision,
        raw_endpoint_host = deployment.endpoint.host,
    );
    let output =
        host_channel::run_script_with_timeout(target, &script, startup_timeout(), runner).await?;
    Ok(report(target, &output, "started"))
}

pub async fn update_reservation(
    target: &ComputeTarget,
    deployment: &Deployment,
    runner: &Runner,
) -> Result<Value, DeployError> {
    safe_runtime(deployment)?;
    let name = shlex_quote(&deployment.name);
    let reservation = Reservation {
        deployment: deployment.name.clone(),
        target: deployment.target.clone(),
        gpu_mode: deployment.resources.gpu_mode.clone(),
        engine: deployment.engine.name.clone(),
        model: deployment.model.repository.clone(),
        revision: deployment.model.revision.clone(),
        endpoint_host: deployment.endpoint.host.clone(),
        port: deployment.endpoint.port,
    };
    let reservation =
        shlex_quote(&STANDARD.encode(
            serde_json::to_vec(&reservation).map_err(|error| DeployError(error.to_string()))?,
        ));
    let script = format!(
        r#"set -euo pipefail
name={name}
path="$HOME/.stado/inference/reservation.json"
if [ ! -f "$path" ] || ! grep -F '"deployment":"'"$name"'"' "$path" >/dev/null; then
  printf 'ERROR\tactive inference reservation does not match %s\n' "$name"; exit 1
fi
temporary="$path.tmp.$$"
trap 'rm -f "$temporary"' EXIT
printf '%s' {reservation} | base64 --decode > "$temporary"
chmod 600 "$temporary"
mv "$temporary" "$path"
trap - EXIT
printf 'STATUS\tupdated\n'
"#
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    Ok(report(target, &output, "updated"))
}
