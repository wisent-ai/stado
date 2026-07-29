#!/bin/bash
set -euo pipefail
exec > /var/log/stado-agent.log 2>&1

echo "Stado AWS agent start: $(date -u)"

WORK=/opt/wisent-agent
rm -rf "$WORK"
mkdir -p "$WORK"
cd "$WORK"

export WC_STORAGE_BACKEND="${WC_STORAGE_BACKEND}"
export WC_BUCKET="${WC_BUCKET}"
export WC_AZURE_STORAGE_ACCOUNT="${WC_AZURE_STORAGE_ACCOUNT}"
export WC_AZURE_CONTAINER="${WC_AZURE_CONTAINER}"
export WC_S3_BUCKET="${WC_S3_BUCKET}"
export WC_S3_REGION="${WC_S3_REGION}"
export WC_LOCAL_STORAGE_PATH="${WC_LOCAL_STORAGE_PATH}"
export WC_BACKUP_STORAGE_BACKEND="${WC_BACKUP_STORAGE_BACKEND}"
export WC_BACKUP_BUCKET="${WC_BACKUP_BUCKET}"
export WC_BACKUP_AZURE_STORAGE_ACCOUNT="${WC_BACKUP_AZURE_STORAGE_ACCOUNT}"
export WC_BACKUP_AZURE_CONTAINER="${WC_BACKUP_AZURE_CONTAINER}"
export WC_BACKUP_S3_REGION="${WC_BACKUP_S3_REGION}"
export WC_BACKUP_LOCAL_STORAGE_PATH="${WC_BACKUP_LOCAL_STORAGE_PATH}"
export AWS_REGION="${AWS_REGION}"
export WC_AGENT_SKARBIEC_URL="${WC_AGENT_SKARBIEC_URL}"
export WC_AGENT_SKARBIEC_CONSUMER="${WC_AGENT_SKARBIEC_CONSUMER}"
export WC_AGENT_SKARBIEC_ITEMS="${WC_AGENT_SKARBIEC_ITEMS}"
export WC_AGENT_SKARBIEC_SECRET_FIELDS="${WC_AGENT_SKARBIEC_SECRET_FIELDS}"
_wc_agent_grant_dir=/run/stado-agent-credentials
_wc_agent_grant_file="$_wc_agent_grant_dir/skarbiec-token"
mkdir -p "$_wc_agent_grant_dir"
chmod u=rwx,go= "$_wc_agent_grant_dir"
umask u=rw,go=
printf '%s' "${STADO_AGENT_SKARBIEC_GRANT_B64}" | base64 --decode > "$_wc_agent_grant_file"
chmod u=rw,go= "$_wc_agent_grant_file"
unset STADO_AGENT_SKARBIEC_GRANT_B64
export WC_AGENT_SKARBIEC_TOKEN_FILE="$_wc_agent_grant_file"
export WC_SKARBIEC_URL="$WC_AGENT_SKARBIEC_URL"
export WC_SKARBIEC_CONSUMER="$WC_AGENT_SKARBIEC_CONSUMER"
export WC_SKARBIEC_TOKEN_FILE="$_wc_agent_grant_file"
case "$WC_SKARBIEC_URL" in
    https://*) ;;
    *) echo "FATAL: WC_AGENT_SKARBIEC_URL must use HTTPS"; false ;;
esac
export WISENT_DTYPE=auto
export PYTORCH_CUDA_ALLOC_CONF=expandable_segments:True
export PYTHONUNBUFFERED=1
export WC_LOCAL_SLOTS=0
export NUMBA_NUM_THREADS=1
export HF_HUB_DOWNLOAD_TIMEOUT=120
export HF_HUB_DISABLE_TELEMETRY=1
export HF_HUB_ETAG_TIMEOUT=1

# Install the exact Rust orchestration release through Stado's public,
# provider-neutral software endpoint. The dispatcher supplies every immutable
# coordinate; missing or malformed coordinates abort startup. Both objects are
# downloaded before install, and a missing checksum entry is a hard failure.
RELEASE_API="${STADO_RELEASE_API_URL}"
RELEASE_VERSION="${STADO_RELEASE_VERSION}"
RELEASE_PLATFORM="${STADO_RELEASE_PLATFORM}"
case "$RELEASE_API" in
    https://*) ;;
    *) echo "FATAL: STADO_RELEASE_API_URL must use HTTPS"; false ;;
esac
case "$RELEASE_VERSION" in
    *[![:alnum:]._-]*|"") echo "FATAL: invalid STADO_RELEASE_VERSION"; false ;;
esac
case "$RELEASE_PLATFORM" in
    *[![:alnum:]._-]*|"") echo "FATAL: invalid STADO_RELEASE_PLATFORM"; false ;;
esac
RELEASE_API="${RELEASE_API%/}"

# Fetch one explicit immutable Python/model runtime bundle through Stado and
# verify its operator-supplied digest before extraction.
RUNTIME_URI="${STADO_AGENT_RUNTIME_BUNDLE_URI}"
RUNTIME_SHA256="${STADO_AGENT_RUNTIME_BUNDLE_SHA256}"
case "$RUNTIME_URI" in
    stado://releases/*/*/*/*) ;;
    *) echo "FATAL: STADO_AGENT_RUNTIME_BUNDLE_URI must be an exact stado://releases/<product>/<version>/<platform>/<object> URI"; false ;;
esac
case "$RUNTIME_SHA256" in
    *[![:xdigit:]]*|"") echo "FATAL: STADO_AGENT_RUNTIME_BUNDLE_SHA256 must be a SHA-256 hex digest"; false ;;
esac
RUNTIME_ARCHIVE="$(mktemp)"
trap 'rm -f "$RUNTIME_ARCHIVE"' EXIT
curl -fsSL --get --data-urlencode "uri=$RUNTIME_URI" \
    "$RELEASE_API/api/release/object" -o "$RUNTIME_ARCHIVE"
printf '%s  %s\n' "$RUNTIME_SHA256" "$RUNTIME_ARCHIVE" | sha256sum -c -
RUNTIME_ROOT="$WORK/runtime"
mkdir -p "$RUNTIME_ROOT"
tar -xzf "$RUNTIME_ARCHIVE" --no-same-owner -C "$RUNTIME_ROOT"
rm -f "$RUNTIME_ARCHIVE"
trap - EXIT
[ -x "$RUNTIME_ROOT/.venv/bin/python" ] || {
    echo "FATAL: immutable agent runtime bundle must contain executable .venv/bin/python"
    false
}
[ -d "$RUNTIME_ROOT/huggingface/hub" ] || {
    echo "FATAL: immutable agent runtime bundle must contain huggingface/hub model cache"
    false
}
export PATH="$RUNTIME_ROOT/.venv/bin:$PATH"
export HF_HOME="$RUNTIME_ROOT/huggingface"
export HF_HUB_OFFLINE=true
export HF_DATASETS_OFFLINE=true
export TRANSFORMERS_OFFLINE=true
AGENT_BIN="$WORK/bin/stado"
_wc_release_get() {
    curl -fsSL --get \
        --data-urlencode "uri=stado://releases/stado/$RELEASE_VERSION/$RELEASE_PLATFORM/$RELEASE_OBJECT" \
        "$RELEASE_API/api/release/object" \
        -o "$RELEASE_DESTINATION"
}
_wc_install_agent_binary() {
    mkdir -p "$WORK/bin" || return
    local tmp rc
    tmp="$(mktemp -d)" || return
    RELEASE_OBJECT=stado
    RELEASE_DESTINATION="$tmp/stado"
    _wc_release_get || { rc=$?; rm -rf "$tmp"; return "$rc"; }
    RELEASE_OBJECT=SHA256SUMS
    RELEASE_DESTINATION="$tmp/SHA256SUMS"
    _wc_release_get || { rc=$?; rm -rf "$tmp"; return "$rc"; }
    grep -E '[ *]stado$' "$tmp/SHA256SUMS" > "$tmp/stado.sha256" || { rc=$?; rm -rf "$tmp"; return "$rc"; }
    (cd "$tmp" && sha256sum -c stado.sha256) || { rc=$?; rm -rf "$tmp"; return "$rc"; }
    chmod u=rwx,go= "$tmp/stado" || { rc=$?; rm -rf "$tmp"; return "$rc"; }
    mv "$tmp/stado" "$WORK/bin/stado" || { rc=$?; rm -rf "$tmp"; return "$rc"; }
    rm -rf "$tmp"
    echo "Installed stado $RELEASE_VERSION ($RELEASE_PLATFORM) -> $WORK/bin/stado"
}
_wc_install_agent_binary
export WC_PYTHON="$RUNTIME_ROOT/.venv/bin/python"

set +e
"$AGENT_BIN" agent --kind "${PROVIDER_KIND}" --gpu-type "${ACCEL_TYPE}" --idle-shutdown
EXIT=$?
echo "Agent exited with $EXIT at $(date -u); provider adapter cleanup remains scheduler-owned"
exit $EXIT
