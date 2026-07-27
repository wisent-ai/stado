#!/bin/bash
set -euxo pipefail
exec > /var/log/stado-agent.log 2>&1

echo "Stado AWS agent start: $(date -u)"
while fuser /var/lib/dpkg/lock-frontend >/dev/null 2>&1 || fuser /var/lib/apt/lists/lock >/dev/null 2>&1; do
    sleep 2
done
apt-get update
apt-get install -y python3-venv python3-pip git ca-certificates curl

WORK=/opt/stado-agent
rm -rf "$WORK"
mkdir -p "$WORK"
cd "$WORK"
python3 -m venv .venv
source .venv/bin/activate
pip install --upgrade pip
pip install --upgrade wisent wisent-extractors wisent-evaluators wisent-tools \
    lm-eval optuna matplotlib word2number evaluate
pip install --upgrade --force-reinstall 'transformers>=4.55,<5.0' 'tokenizers>=0.20,<0.22'
pip install --upgrade --force-reinstall 'datasets>=2.18,<3.0' 'huggingface-hub>=0.34.0,<1.0'
if pip show hf-xet >/dev/null 2>&1; then pip uninstall -y hf-xet; fi

export WC_STORAGE_BACKEND=s3
export WC_S3_BUCKET="${WC_S3_BUCKET}"
export WC_S3_REGION="${AWS_REGION}"
export WC_BUCKET="${WC_S3_BUCKET}"
export AWS_REGION="${AWS_REGION}"
export HF_TOKEN="${HF_TOKEN:-}"
export HUGGING_FACE_HUB_TOKEN="${HF_TOKEN:-}"
export SUPABASE_ACCESS_TOKEN="${WC_SUPABASE_TOKEN:-}"
export WISENT_DTYPE=auto
export PYTORCH_CUDA_ALLOC_CONF=expandable_segments:True
export PYTHONUNBUFFERED=1
export WC_LOCAL_SLOTS=0
export NUMBA_NUM_THREADS=1
export HF_HUB_DOWNLOAD_TIMEOUT=120
export HF_HUB_DISABLE_TELEMETRY=1
export HF_HUB_ETAG_TIMEOUT=1

# Install the Rust orchestration binary from the release bucket. Job
# payloads still run as Python from the venv above (exported as WC_PYTHON
# for the agent's probes), but the control plane has no Python fallback.
# No gcloud on this image, so the download uses the public GCS HTTPS
# endpoint. An unavailable or invalid release aborts startup. Shell-locals
# use $VAR (never the braced form) so the dispatcher's placeholder
# substitution leaves them alone.
AGENT_BIN="$WORK/bin/stado"
_wc_install_agent_binary() {
    mkdir -p "$WORK/bin" || return 1
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
    mv "$tmp/stado" "$WORK/bin/stado" || { rm -rf "$tmp"; return 1; }
    rm -rf "$tmp"
    echo "Installed stado $version (linux-amd64) -> $WORK/bin/stado"
}
_wc_install_agent_binary
export WC_PYTHON="$WORK/.venv/bin/python"

set +e
"$AGENT_BIN" agent --kind aws --gpu-type "${ACCEL_TYPE}" --idle-shutdown
EXIT=$?
echo "Agent exited with $EXIT at $(date -u); stopping instance"
shutdown -h now
exit $EXIT
