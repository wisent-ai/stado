//! Narrow remote lifecycle for one digest-pinned vLLM container.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};

use super::{host_channel, shlex_quote, DeployError, Runner};
use crate::inference::{reservation::Reservation, schema::Deployment};
use crate::targets::ComputeTarget;

fn report(target: &ComputeTarget, output: &super::CommandOutput, ok: &str) -> Value {
    let mut body = host_channel::base_report(target);
    host_channel::finish_report(&mut body, output, ok, "inference operation failed");
    body.insert("stdout".to_string(), Value::String(output.stdout.clone()));
    Value::Object(body)
}

fn unit_name(name: &str) -> String {
    format!("stado-inference-{name}.service")
}

fn safe_runtime(deployment: &Deployment) -> Result<(), DeployError> {
    crate::inference::schema::validate(&json!({
        "schema_version": crate::targets::REGISTRY_SCHEMA_VERSION,
        "targets": [{
            "name": deployment.target,
            "kind": "local",
            "gpu_type": "declared",
            "vram_gb": u8::MAX,
        }],
        "inference": {"deployments": [deployment], "routes": {}}
    }))
    .map_err(DeployError)
}

pub async fn inventory(target: &ComputeTarget, runner: &Runner) -> Result<Value, DeployError> {
    let script = r#"set -euo pipefail
printf 'HOST\t'; hostname
printf 'KERNEL\t'; uname -sr
if ! command -v nvidia-smi >/dev/null; then printf 'ERROR\tnvidia-smi missing\n'; exit 1; fi
printf 'GPU\t'; nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv,noheader
printf 'CUDA_CAPABILITY\t'; nvidia-smi --query-gpu=compute_cap --format=csv,noheader
printf 'RAM\t'; free -b | grep '^Mem:'
if ! command -v tailscale >/dev/null; then printf 'ERROR\ntailscale missing\n'; false; fi
printf 'TAILSCALE\t'
tailscale ip | while IFS= read -r address; do case "$address" in *:*) ;; *) printf '%s\n' "$address"; break ;; esac; done
printf 'PROCESSES\t'; nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader || true
if ! command -v docker >/dev/null; then printf 'ERROR\tdocker missing\n'; exit 1; fi
printf 'DOCKER\t'; docker version --format '{{.Server.Version}}'
runtimes=$(docker info --format '{{json .Runtimes}}')
printf 'DOCKER_RUNTIMES\t%s\n' "$runtimes"
case "$runtimes" in *nvidia*) ;; *) printf 'ERROR\tDocker NVIDIA runtime missing\n'; false ;; esac
printf 'DISK\t'; df -Pk "$HOME" | tail -n 1
"#;
    let output = host_channel::run_script(target, script, runner).await?;
    Ok(report(target, &output, "inventoried"))
}

/// Large immutable image pulls and first model loads need a wider bound than
/// ordinary host operations; connection establishment keeps its short SSH cap.
pub fn startup_timeout() -> std::time::Duration {
    host_channel::remote_timeout().saturating_mul(u8::BITS.saturating_mul(u8::BITS))
}

pub async fn install(
    target: &ComputeTarget,
    deployment: &Deployment,
    api_key: &str,
    huggingface_token: Option<&str>,
    runner: &Runner,
) -> Result<Value, DeployError> {
    safe_runtime(deployment)?;
    if api_key.is_empty() || api_key.chars().any(char::is_control) {
        return Err(DeployError(
            "inference bearer must be non-empty and single-line".to_string(),
        ));
    }
    if huggingface_token
        .is_some_and(|token| token.is_empty() || token.chars().any(char::is_control))
    {
        return Err(DeployError(
            "Hugging Face token must be non-empty and single-line".to_string(),
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
    let huggingface_secret = huggingface_token
        .map(|token| shlex_quote(&STANDARD.encode(token)))
        .unwrap_or_else(|| "''".to_string());
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
if nvidia-smi --query-compute-apps=pid --format=csv,noheader,nounits | grep -E '[0-9]' >/dev/null; then
  if ! docker ps --filter "name=^stado-inference-$name$" --format '{{{{.Names}}}}' | grep -Fx "stado-inference-$name" >/dev/null; then
    printf 'ERROR\tGPU has an unmanaged active compute process\n'; exit 1
  fi
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
if [ -n {huggingface_secret} ]; then
  printf 'HF_TOKEN=' >> "$root/runtime.env"
  printf '%s' {huggingface_secret} | base64 --decode >> "$root/runtime.env"
  printf '\n' >> "$root/runtime.env"
fi
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

pub async fn status(
    target: &ComputeTarget,
    deployment: &Deployment,
    runner: &Runner,
) -> Result<Value, DeployError> {
    safe_runtime(deployment)?;
    let name = shlex_quote(&deployment.name);
    let script = format!(
        r#"set -u
name={name}
printf 'CONTAINER\t'; docker inspect --format '{{{{.State.Status}}}}' "stado-inference-$name" || printf 'missing\n'
printf 'GPU\t'; nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader || true
"#
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    Ok(report(target, &output, "reported"))
}

pub async fn probe(
    target: &ComputeTarget,
    deployment: &Deployment,
    api_key: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    safe_runtime(deployment)?;
    let secret = shlex_quote(&STANDARD.encode(api_key));
    let name = shlex_quote(&deployment.name);
    let port = deployment.endpoint.port;
    let endpoint_host = shlex_quote(&deployment.endpoint.host);
    let script = format!(
        r#"set -euo pipefail
token=$(printf '%s' {secret} | base64 --decode)
name={name}
state=$(docker inspect --format '{{{{.State.Status}}}}' "stado-inference-$name") || {{ printf 'ERROR\tinference container missing\n'; false; }}
if [ "$state" != running ]; then
  printf 'ERROR\tinference container is %s\n' "$state"; false
fi
endpoint_host={endpoint_host}
curl --fail --silent --show-error --max-time $(printf '%s' '15') -H "Authorization: Bearer $token" "http://$endpoint_host:{port}/v1/models" >/dev/null
printf 'READY\tauthenticated\n'
"#
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    Ok(report(target, &output, "ready"))
}

pub async fn verify_completion(
    target: &ComputeTarget,
    deployment: &Deployment,
    api_key: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    safe_runtime(deployment)?;
    let secret = shlex_quote(&STANDARD.encode(api_key));
    let payload = shlex_quote(
        &json!({
            "model": deployment.name,
            "messages": [{"role": "user", "content": "Reply with the single word ready."}],
            "max_tokens": u8::BITS,
        })
        .to_string(),
    );
    let port = deployment.endpoint.port;
    let endpoint_host = shlex_quote(&deployment.endpoint.host);
    let script = format!(
        r#"set -euo pipefail
token=$(printf '%s' {secret} | base64 --decode)
endpoint_host={endpoint_host}
curl --fail --silent --show-error --max-time $(printf '%s' '120') -H "Authorization: Bearer $token" -H 'Content-Type: application/json' --data {payload} "http://$endpoint_host:{port}/v1/chat/completions"
printf '\n'
"#
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    Ok(report(target, &output, "verified"))
}

pub async fn logs(
    target: &ComputeTarget,
    deployment: &Deployment,
    lines: usize,
    runner: &Runner,
) -> Result<Value, DeployError> {
    safe_runtime(deployment)?;
    let name = shlex_quote(&deployment.name);
    let script =
        format!("set -euo pipefail\ndocker logs --tail {lines} \"stado-inference-{name}\" 2>&1\n");
    let output = host_channel::run_script(target, &script, runner).await?;
    Ok(report(target, &output, "read"))
}

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
