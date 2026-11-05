//! Read-only host inventory, container state, endpoint probes and log tails.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};

use crate::deploy::inference::support::{report, safe_runtime};
use crate::deploy::{host_channel, shlex_quote, DeployError, Runner};
use crate::inference::schema::Deployment;
use crate::targets::ComputeTarget;

/// The host's inventory. `ADDRESSES` lists every global IPv4 address of the
/// host's interfaces that are up (`ip -4 -o addr show scope global up`),
/// space-separated after the tab; the plan picks the one its visibility binds
/// on. The host needs no Tailscale client: a host that has one reports its
/// tailnet address among the others, because the client puts it on an
/// interface.
pub async fn inventory(target: &ComputeTarget, runner: &Runner) -> Result<Value, DeployError> {
    let script = r#"set -euo pipefail
printf 'HOST\t'; hostname
printf 'KERNEL\t'; uname -sr
if ! command -v nvidia-smi >/dev/null; then printf 'ERROR\tnvidia-smi missing\n'; exit 1; fi
printf 'GPU\t'; nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv,noheader
printf 'CUDA_CAPABILITY\t'; nvidia-smi --query-gpu=compute_cap --format=csv,noheader
printf 'RAM\t'; free -b | grep '^Mem:'
if ! command -v ip >/dev/null; then printf 'ERROR\tip (iproute2) missing, so the host addresses cannot be read\n'; exit 1; fi
printf 'ADDRESSES\t'; ip -4 -o addr show scope global up | while read -r _ _ _ cidr _; do printf '%s ' "${cidr%/*}"; done; printf '\n'
printf 'PROCESSES\t'; nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader || true
printf 'BOARDS\t'; nvidia-smi --query-gpu=uuid,memory.total,memory.free --format=csv,noheader,nounits | while IFS= read -r board; do printf '%s;' "$board"; done; printf '\n'
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
curl --fail --silent --show-error -H "Authorization: Bearer $token" "http://$endpoint_host:{port}"{ready_path} >/dev/null
printf 'READY\tauthenticated\n'
"#,
        ready_path = shlex_quote(match &deployment.engine.declared {
            Some(declared) => declared.ready_path.as_str(),
            None => "/v1/models",
        })
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
    // One generation of the deployment's own kind: a chat completion from
    // vLLM, or the request the declared engine names for its image, video
    // or speech.
    let (path, body) = match &deployment.engine.declared {
        Some(declared) => (
            declared.generate_path.clone(),
            declared.generate_body.clone(),
        ),
        None => (
            "/v1/chat/completions".to_string(),
            json!({
                "model": deployment.name,
                "messages": [{"role": "user", "content": "Reply with the single word ready."}],
            }),
        ),
    };
    let payload = shlex_quote(&body.to_string());
    let path = shlex_quote(&path);
    let port = deployment.endpoint.port;
    let endpoint_host = shlex_quote(&deployment.endpoint.host);
    // A text answer is printed, so the operator reads what the model said;
    // an image, video or audio answer is binary, so its status and size are.
    let output_flags = match &deployment.engine.declared {
        Some(_) => {
            "--output /dev/null --write-out 'GENERATED\\t%{http_code}\\t%{size_download} bytes\\n'"
        }
        None => "",
    };
    let script = format!(
        r#"set -euo pipefail
token=$(printf '%s' {secret} | base64 --decode)
endpoint_host={endpoint_host}
curl --fail --silent --show-error {output_flags} -H "Authorization: Bearer $token" -H 'Content-Type: application/json' --data {payload} "http://$endpoint_host:{port}"{path}
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
