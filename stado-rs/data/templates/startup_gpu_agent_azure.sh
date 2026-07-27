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

export HF_TOKEN="${HF_TOKEN}"
export HUGGING_FACE_HUB_TOKEN="${HF_TOKEN}"
export WISENT_DTYPE=auto
export PYTORCH_CUDA_ALLOC_CONF=expandable_segments:True
export PYTHONUNBUFFERED=1
export WC_LOCAL_SLOTS=0
export NUMBA_NUM_THREADS=1
export HF_HUB_DOWNLOAD_TIMEOUT=120
export HF_HUB_DISABLE_TELEMETRY=1
export HF_HUB_ETAG_TIMEOUT=1

# Storage backend selection. Set on the dispatcher's render path; see
# config.WC_STORAGE_BACKEND. When unset/"gcs" the agent writes to GCS via
# the gsutil/SDK path (requires GCP service-account JSON; not configured
# in this template by default). When "azure" the agent writes to Azure
# Blob via DefaultAzureCredential (managed identity on the VM is the
# natural fit; granted Storage Blob Data Contributor on the container).
export WC_STORAGE_BACKEND="${WC_STORAGE_BACKEND}"
export WC_AZURE_STORAGE_ACCOUNT="${WC_AZURE_STORAGE_ACCOUNT}"
export WC_AZURE_CONTAINER="${WC_AZURE_CONTAINER}"

# Pre-warm the small auxiliary models so each claimed job skips the download.
huggingface-cli download cross-encoder/nli-deberta-v3-small || true
huggingface-cli download sentence-transformers/all-MiniLM-L6-v2 || true

# Install the Rust orchestration binary from the release bucket. Job
# payloads still run as Python from the venv above (exported as WC_PYTHON
# for the agent's probes), but the control plane has no Python fallback.
# gcloud/gsutil are not installed on this image, so the download uses the
# public GCS HTTPS endpoint. An unavailable or invalid release aborts
# startup. Shell-locals use $VAR (never the braced form) so the dispatcher's
# placeholder substitution leaves them alone.
AGENT_BIN=/opt/wisent-agent/bin/stado
_wc_install_agent_binary() {
    mkdir -p /opt/wisent-agent/bin || return 1
    local version
    version="$(curl -fsSL https://storage.googleapis.com/wisent-compute/releases/stado/latest.json 2>/dev/null \
        | python3 -c 'import json,sys; sys.stdout.write(json.load(sys.stdin)["version"])')" || return 1
    [ -n "$version" ] || return 1
    local base="https://storage.googleapis.com/wisent-compute/releases/stado/$version/linux-amd64"
    local tmp
    tmp="$(mktemp -d)" || return 1
    curl -fsSL "$base/stado" -o "$tmp/stado" 2>/dev/null || { rm -rf "$tmp"; return 1; }
    curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS" 2>/dev/null || { rm -rf "$tmp"; return 1; }
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
