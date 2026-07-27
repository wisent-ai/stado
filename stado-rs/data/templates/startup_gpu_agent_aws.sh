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
export WISENT_DTYPE=auto
export PYTORCH_CUDA_ALLOC_CONF=expandable_segments:True
export PYTHONUNBUFFERED=1
export WC_LOCAL_SLOTS=0
export NUMBA_NUM_THREADS=1
export HF_HUB_DOWNLOAD_TIMEOUT=120
export HF_HUB_DISABLE_TELEMETRY=1
export HF_HUB_ETAG_TIMEOUT=1

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
# them alone; the two below drop the WC_ prefix because they need braced
# ${VAR%...} operators, which must not look like a dispatcher key.
#
# An EC2 instance has no Azure identity to mint a bearer token with, so
# an Azure blob channel must be public-read or carry a container SAS in
# WC_RELEASE_BASE_URL. That query string is split off the base here and
# re-appended after each object path, which is the only place it works.
RELEASE_BASE="${WC_RELEASE_BASE_URL}"
RELEASE_QS=""
case "$RELEASE_BASE" in
    *\?*)
        RELEASE_QS="?${RELEASE_BASE#*\?}"
        RELEASE_BASE="${RELEASE_BASE%%\?*}"
        ;;
esac
RELEASE_BASE="${RELEASE_BASE%/}"
AGENT_BIN="$WORK/bin/stado"
# curl against the release channel: $1 is the URL, remaining args are
# forwarded to curl, and any pre-authentication query string rides along.
_wc_release_curl() {
    local url="$1"
    shift
    curl -fsSL "$url$RELEASE_QS" "$@"
}
_wc_install_agent_binary() {
    mkdir -p "$WORK/bin" || return 1
    local version rc
    version="$(_wc_release_curl "$RELEASE_BASE/latest.json" \
        | python3 -c 'import json,sys; sys.stdout.write(json.load(sys.stdin)["version"])')" || return 1
    [ -n "$version" ] || return 1
    local base="$RELEASE_BASE/$version/linux-amd64"
    local tmp
    tmp="$(mktemp -d)" || return 1
    _wc_release_curl "$base/stado" -o "$tmp/stado" || { rc=$?; rm -rf "$tmp"; return "$rc"; }
    _wc_release_curl "$base/SHA256SUMS" -o "$tmp/SHA256SUMS" || { rc=$?; rm -rf "$tmp"; return "$rc"; }
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
