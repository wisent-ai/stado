#!/bin/bash
# Azure VM cloud-init template: install wisent-compute, start the agent in
# idle-shutdown mode. Mirrors startup_gpu_agent.sh (the GCP variant) but with
# Azure-side bootstrap. The agent reads its own VRAM via nvidia-smi and packs
# as many queued jobs as fit — no constant slot count. Self-deletes the VM
# when the queue stops yielding eligible work.
set -euxo pipefail
exec > /var/log/wisent-agent.log 2>&1

echo "Wisent Azure agent VM start: $(date -u)"

# microsoft-dsvm:ubuntu-hpc:2204 ships with NVIDIA driver + CUDA preinstalled,
# matching deeplearning-platform-release on GCP. We still install python venv
# tooling because the DSVM's system Python is not what we want to pollute.
while fuser /var/lib/dpkg/lock-frontend >/dev/null 2>&1 || fuser /var/lib/apt/lists/lock >/dev/null 2>&1; do
    echo "Waiting for apt lock..."
done
apt-get update
apt-get install -y python3-venv python3-pip git ca-certificates curl gnupg

WORK=/opt/wisent-agent
rm -rf "$WORK"
mkdir -p "$WORK"
cd "$WORK"
python3 -m venv .venv
source .venv/bin/activate
pip install --upgrade pip
# Storage access is implemented by the Rust agent; the Python environment
# only contains job-runtime packages.
pip install --upgrade wisent wisent-extractors wisent-evaluators wisent-tools \
    lm-eval optuna matplotlib word2number evaluate
pip install --upgrade --force-reinstall 'transformers>=4.55,<5.0' 'tokenizers>=0.20,<0.22'
pip install --upgrade --force-reinstall 'datasets>=2.18,<3.0' 'huggingface-hub>=0.34.0,<1.0'
pip uninstall -y hf-xet || true

export WISENT_DTYPE=auto
export PYTORCH_CUDA_ALLOC_CONF=expandable_segments:True
export PYTHONUNBUFFERED=1
export WC_LOCAL_SLOTS=0
export NUMBA_NUM_THREADS=1
export HF_HUB_DOWNLOAD_TIMEOUT=120
export HF_HUB_DISABLE_TELEMETRY=1
export HF_HUB_ETAG_TIMEOUT=1

# Azure is the only primary in this template. Blob access comes from the
# user-assigned managed identity attached by providers/azure/mod.rs; no cloud
# CLI, service-principal environment or GCP credential is installed.
export WC_STORAGE_BACKEND="${WC_STORAGE_BACKEND}"
export WC_AZURE_STORAGE_ACCOUNT="${WC_AZURE_STORAGE_ACCOUNT}"
export WC_AZURE_CONTAINER="${WC_AZURE_CONTAINER}"
[ "$WC_STORAGE_BACKEND" = "azure" ] || {
    echo "FATAL: Azure agent rendered with WC_STORAGE_BACKEND=$WC_STORAGE_BACKEND (expected azure)" | tee /dev/stderr
    false
}
[ -n "$WC_AZURE_STORAGE_ACCOUNT" ] || {
    echo "FATAL: WC_AZURE_STORAGE_ACCOUNT is unresolved; provision the Azure account and set storage.azure.account" | tee /dev/stderr
    false
}

# S3 is read failover and a synchronous replica, never an alternate writer.
# The VM gets only the stado-azure-agent grant whose exact item allowlist was
# verified by the coordinator. A single-read FIFO transfers it into the Rust
# agent's in-memory cache; the path then disappears. Raw values are never
# rendered into cloud-init or inherited by workload processes.
export WC_BACKUP_STORAGE_BACKEND="${WC_BACKUP_STORAGE_BACKEND}"
export WC_BACKUP_BUCKET="${WC_BACKUP_BUCKET}"
export WC_BACKUP_S3_REGION="${WC_BACKUP_S3_REGION}"
export WC_SKARBIEC_URL="${WC_AGENT_SKARBIEC_URL}"
export WC_SKARBIEC_CONSUMER="${WC_AGENT_SKARBIEC_CONSUMER}"
_wc_agent_grant_dir=/run/stado-agent-credentials
_wc_agent_grant_fifo="$_wc_agent_grant_dir/skarbiec-token"
export WC_SKARBIEC_TOKEN_FILE="$_wc_agent_grant_fifo"
[ "$WC_BACKUP_STORAGE_BACKEND" = "s3" ] || {
    echo "FATAL: Azure agent requires WC_BACKUP_STORAGE_BACKEND=s3 for read failover" | tee /dev/stderr
    false
}
[ -n "$WC_BACKUP_BUCKET" ] && [ -n "$WC_BACKUP_S3_REGION" ] || {
    echo "FATAL: S3 backup bucket/region unresolved; set WC_BACKUP_BUCKET and WC_BACKUP_S3_REGION" | tee /dev/stderr
    false
}
case "$WC_SKARBIEC_URL" in
    https://*) ;;
    *)
        echo "FATAL: WC_AGENT_SKARBIEC_URL must be an HTTPS endpoint reachable from this VM" | tee /dev/stderr
        false
        ;;
esac
mkdir -p "$_wc_agent_grant_dir"
chmod u=rwx,go= "$_wc_agent_grant_dir"
rm -f "$_wc_agent_grant_fifo"
mkfifo "$_wc_agent_grant_fifo"
chmod u=rw,go= "$_wc_agent_grant_fifo"
set +x
_wc_agent_grant="${WC_AGENT_SKARBIEC_TOKEN}"
(
    set +x
    printf '%s' "$_wc_agent_grant" > "$_wc_agent_grant_fifo"
    rm -f "$_wc_agent_grant_fifo"
    rmdir "$_wc_agent_grant_dir" || true
) &
unset _wc_agent_grant
set -x

# Pre-warm the small auxiliary models so each claimed job skips the download.
huggingface-cli download cross-encoder/nli-deberta-v3-small || true
huggingface-cli download sentence-transformers/all-MiniLM-L6-v2 || true

# Install the Rust orchestration binary from the release channel. Job
# payloads still run as Python from the venv above (exported as WC_PYTHON
# for the agent's probes), but the control plane has no Python fallback.
# The channel base is substituted by the dispatcher from
# config::release_base_url() (env WC_RELEASE_BASE_URL), so this template
# is not tied to any one cloud's object store. No cloud CLI is installed
# on this image, so every download is plain curl over HTTPS. An
# unavailable or invalid release aborts startup. curl's stderr is NOT
# discarded: a failed release download is the difference between a
# working fleet and a silently empty one. Shell-locals use $VAR (never
# the braced form) so the dispatcher's placeholder substitution leaves
# them alone.
WC_RELEASE_BASE="${WC_RELEASE_BASE_URL}"
AGENT_BIN=/opt/wisent-agent/bin/stado
# curl against the release channel: $1 is the URL, remaining args are
# forwarded to curl. An Azure blob channel is not public-read, so it gets
# a managed-identity bearer token for the storage audience -- the same
# audience and REST API version the agent's own blob client pins. That
# requires the VM to carry a user-assigned identity; without one the
# token fetch fails and startup aborts rather than installing nothing.
# Non-Azure hosts are fetched anonymously only when an explicitly configured
# provider-neutral HTTP channel is used; this Azure path has no GCS fallback.
# Tracing is suppressed across the token's lifetime so the bearer never
# reaches /var/log/wisent-agent.log.
_wc_release_curl() {
    local url="$1"
    shift
    case "$url" in
        https://*.blob.core.windows.net/*) ;;
        *)
            curl -fsSL "$url" "$@"
            return $?
            ;;
    esac
    local token status
    set +x
    token="$(curl -fsSL -H 'Metadata: true' \
        'http://169.254.169.254/metadata/identity/oauth2/token?api-version=2018-02-01&resource=https://storage.azure.com' \
        | python3 -c 'import json,sys; sys.stdout.write(json.load(sys.stdin)["access_token"])')" || token=""
    curl -fsSL -H "Authorization: Bearer $token" -H 'x-ms-version: 2023-11-03' "$url" "$@"
    status=$?
    token=""
    set -x
    return "$status"
}
_wc_install_agent_binary() {
    mkdir -p /opt/wisent-agent/bin || return 1
    local version rc
    version="$(_wc_release_curl "$WC_RELEASE_BASE/latest.json" \
        | python3 -c 'import json,sys; sys.stdout.write(json.load(sys.stdin)["version"])')" || return 1
    [ -n "$version" ] || return 1
    local base="$WC_RELEASE_BASE/$version/linux-amd64"
    local tmp
    tmp="$(mktemp -d)" || return 1
    _wc_release_curl "$base/stado" -o "$tmp/stado" || { rc=$?; rm -rf "$tmp"; return "$rc"; }
    _wc_release_curl "$base/SHA256SUMS" -o "$tmp/SHA256SUMS" || { rc=$?; rm -rf "$tmp"; return "$rc"; }
    grep -E '[ *]stado$' "$tmp/SHA256SUMS" > "$tmp/stado.sha256" || { rm -rf "$tmp"; return 1; }
    (cd "$tmp" && sha256sum -c stado.sha256) || { rm -rf "$tmp"; return 1; }
    chmod 755 "$tmp/stado" || { rm -rf "$tmp"; return 1; }
    mv "$tmp/stado" /opt/wisent-agent/bin/stado || { rm -rf "$tmp"; return 1; }
    rm -rf "$tmp"
    echo "Installed stado $version (linux-amd64) -> /opt/wisent-agent/bin/stado"
}
_wc_install_agent_binary
export WC_PYTHON=/opt/wisent-agent/.venv/bin/python

# Run the agent. --idle-shutdown makes it exit + self-delete when no queued
# job is eligible for this VM's free VRAM. The agent broadcasts capacity to
# whichever storage backend WC_STORAGE_BACKEND selects.
"$AGENT_BIN" agent --kind azure --gpu-type "${ACCEL_TYPE}" --idle-shutdown
EXIT=$?
echo "Agent exited with $EXIT at $(date -u)"
