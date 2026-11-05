//! Container start and the single-writer reservation record it publishes.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::Value;

use crate::deploy::inference::support::{report, safe_runtime, unit_name};
use crate::deploy::{host_channel, shlex_quote, DeployError, Runner};
use crate::inference::{reservation::Reservation, schema::Deployment};
use crate::targets::ComputeTarget;

/// The environment variable the engine reads its bearer from: vLLM's own,
/// or the one the declared engine names.
pub(crate) fn key_variable(deployment: &Deployment) -> String {
    match &deployment.engine.declared {
        Some(declared) => declared.key_variable.clone(),
        None => "VLLM_API_KEY".to_string(),
    }
}

/// The image and arguments `docker run` is given, each word quoted. vLLM is
/// started as Stado always started it; a declared engine gets its declared
/// arguments, with `{model}`, `{revision}`, `{name}`, `{host}` and `{port}`
/// replaced by the deployment's values.
fn engine_command(deployment: &Deployment, max_model_len: u64, kv_cache: &str) -> String {
    let port = deployment.endpoint.port.to_string();
    let words: Vec<String> = match &deployment.engine.declared {
        Some(declared) => std::iter::once(deployment.engine.image.clone())
            .chain(declared.args.iter().map(|arg| {
                arg.replace("{model}", &deployment.model.repository)
                    .replace("{revision}", &deployment.model.revision)
                    .replace("{name}", &deployment.name)
                    .replace("{host}", &deployment.endpoint.host)
                    .replace("{port}", &port)
            }))
            .collect(),
        None => {
            let mut words = vec![
                deployment.engine.image.clone(),
                "--model".to_string(),
                deployment.model.repository.clone(),
                "--revision".to_string(),
                deployment.model.revision.clone(),
                "--served-model-name".to_string(),
                deployment.name.clone(),
                "--host".to_string(),
                deployment.endpoint.host.clone(),
                "--port".to_string(),
                port,
                "--max-model-len".to_string(),
                max_model_len.to_string(),
            ];
            words.extend(kv_cache.split_whitespace().map(str::to_string));
            words.extend(
                ["--enable-auto-tool-choice", "--tool-call-parser", "hermes"].map(str::to_string),
            );
            words
        }
    };
    words
        .iter()
        .map(|word| shlex_quote(word))
        .collect::<Vec<_>>()
        .join(" ")
}

pub async fn install(
    target: &ComputeTarget,
    deployment: &Deployment,
    api_key: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    safe_runtime(deployment)?;
    if api_key.is_empty() || api_key.chars().any(char::is_control) {
        return Err(
            DeployError("inference bearer must be non-empty and single-line".to_string())
                .stating(crate::primitives::failure::FailureCode::Refused),
        );
    }
    let name = shlex_quote(&deployment.name);
    let image = shlex_quote(&deployment.engine.image);
    let endpoint_host = shlex_quote(&deployment.endpoint.host);
    let port = deployment.endpoint.port;
    let max_model_len = deployment.resources.max_model_len;
    // A deployment that states no KV-cache budget passes none, and the engine
    // sizes its own; one that states it in GiB is handed bytes.
    let kv_cache_argument = match deployment.resources.kv_cache_memory_gb {
        Some(gib) => format!(
            " --kv-cache-memory {}",
            u128::from(gib) * crate::providers::local::host_memory::constants::GIB as u128
        ),
        None => String::new(),
    };
    // A deployment that names no cache directory keeps its cache under the
    // deployment's own root on the host.
    let (cache_dir, cache_mount) = match deployment.resources.cache_dir.as_deref() {
        Some(path) => (shlex_quote(path), format!("{path}:/data/huggingface")),
        None => (
            "\"$root/cache\"".to_string(),
            "\"$root/cache:/data/huggingface\"".to_string(),
        ),
    };
    let secret = shlex_quote(&STANDARD.encode(api_key));
    let unit = shlex_quote(&unit_name(&deployment.name));
    let reservation = Reservation {
        deployment: deployment.name.clone(),
        target: deployment.target.clone(),
        gpu_mode: deployment.resources.gpu_mode,
        engine: deployment.engine.name.clone(),
        model: deployment.model.repository.clone(),
        revision: deployment.model.revision.clone(),
        endpoint_host: deployment.endpoint.host.clone(),
        port: deployment.endpoint.port,
        gpu: deployment.resources.gpu.clone(),
    };
    // The engine sees only the board it was planned on; a deployment that
    // names none sees every board, as it always did. Docker's device request
    // takes the driver UUID verbatim (`device=GPU-…`), quoted for the shell
    // because the value carries `=` and Docker's own `"` quoting.
    let gpus = match deployment.resources.gpu.as_deref() {
        Some(uuid) => shlex_quote(&format!("\"device={uuid}\"")),
        None => "all".to_string(),
    };
    let reservation =
        shlex_quote(&STANDARD.encode(
            serde_json::to_vec(&reservation).map_err(|error| DeployError(error.to_string()))?,
        ));
    // A compute process blocks deployment when no declared service accounts
    // for it. Preserve unknown workloads rather than classifying all existing
    // GPU use as unmanaged or terminating it.
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
printf '%s=' {key_variable} > "$root/runtime.env"
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
container=$(docker run --detach --restart unless-stopped --name "stado-inference-$name" --gpus {gpus} --network host --ipc host --env-file "$root/runtime.env" -v {cache_mount} {engine_command})
printf 'CONTAINER\t%s\n' "$container"
printf 'STATUS\tstarted\n'
"#,
        key_variable = shlex_quote(&key_variable(deployment)),
        engine_command = engine_command(deployment, max_model_len, &kv_cache_argument),
    );
    let output = host_channel::run_script(target, &script, runner).await?;
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
        gpu_mode: deployment.resources.gpu_mode,
        engine: deployment.engine.name.clone(),
        model: deployment.model.repository.clone(),
        revision: deployment.model.revision.clone(),
        endpoint_host: deployment.endpoint.host.clone(),
        port: deployment.endpoint.port,
        gpu: deployment.resources.gpu.clone(),
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
