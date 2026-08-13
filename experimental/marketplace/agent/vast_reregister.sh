#!/usr/bin/env bash
# Delete a stale/unauthenticated Vast machine record then re-register cleanly.
#
# Follows Jay (Vast support)'s guidance: when /daemon/identify/ returns
# "machine_api_key is too old or does not have machine registration rights"
# and the machine cannot be listed (unauthenticated_machine), the fix is to
# delete the machine on Vast then re-run the canonical install flow so a new
# machine_api_key gets minted with the right server-side state.
#
# Usage:
#   sudo VAST_HOST_API_KEY=... OLD_MACHINE_ID=80479 PRICE_GPU=0.80 bash vast_reregister.sh
#
# Env:
#   VAST_HOST_API_KEY   user/team API key with machine_write (required)
#   OLD_MACHINE_ID      existing machine id to delete before re-register
#                       (optional; skipped if unset)
#   PRICE_GPU           on-demand price per GPU in USD/hour for the new
#                       listing (optional; skipped if unset)
#   PRICE_DISK          storage price in USD/GB/month (default 0.10)
#   PRICE_INETU         internet upload price in USD/TB (default 3.0)
#   PRICE_INETD         internet download price in USD/TB (default 2.0)
#   MIN_CHUNK           minimum GPU count per rental (default 1)
#   VAST_SERVER         API base (default https://console.vast.ai)

set -euo pipefail

: "${VAST_HOST_API_KEY:?VAST_HOST_API_KEY is required}"
OLD_MACHINE_ID="${OLD_MACHINE_ID:-}"
PRICE_GPU="${PRICE_GPU:-}"
PRICE_DISK="${PRICE_DISK:-0.10}"
PRICE_INETU="${PRICE_INETU:-3.0}"
PRICE_INETD="${PRICE_INETD:-2.0}"
MIN_CHUNK="${MIN_CHUNK:-1}"
VAST_SERVER="${VAST_SERVER:-https://console.vast.ai}"

if [[ $EUID -ne 0 ]]; then
    echo "run as root" >&2
    exit 1
fi

command -v curl        >/dev/null || { echo "curl missing"        >&2; exit 1; }
command -v python3     >/dev/null || { echo "python3 missing"     >&2; exit 1; }
command -v systemctl   >/dev/null || { echo "systemctl missing"   >&2; exit 1; }

log() { printf '[%s] %s\n' "$(date -u +%FT%TZ)" "$*"; }

if [[ -n "$OLD_MACHINE_ID" ]]; then
    log "deleting old machine id $OLD_MACHINE_ID via force_delete"
    resp="$(curl -sS -X POST \
        -H "Authorization: Bearer $VAST_HOST_API_KEY" \
        "$VAST_SERVER/api/v0/machines/${OLD_MACHINE_ID}/force_delete/")"
    log "force_delete response: $resp"
fi

# Fetch the canonical installer. The one that does the /daemon/identify/
# handshake and wires kaalia with the server-issued nonce.
INSTALLER=/tmp/vast_install.py
log "fetching installer from ${VAST_SERVER}/install"
curl -fsSL "${VAST_SERVER}/install" -o "$INSTALLER"

# Ensure we start with a clean machine_id file so --reset-machine actually
# mints a fresh hex that the server has never seen before.
rm -f /var/lib/vastai_kaalia/machine_id /tmp/machine_id || true

log "running installer (skipping already-provisioned driver / docker / libvirt / partitioning)"
cd /tmp
python3 "$INSTALLER" "$VAST_HOST_API_KEY" \
    --no-driver --no-partitioning --no-docker --no-libvirt --reset-machine

log "installer exited ok; restarting vastai.service"
systemctl restart vastai

# Give kaalia a moment to finish Identify + first ContainerList exchange.
for i in $(seq 1 30); do
    if grep -q "handle_message mtype: Command" /var/lib/vastai_kaalia/kaalia.log 2>/dev/null; then
        log "kaalia received server Command (setup dispatched)"
        break
    fi
    sleep 2
done

# Query the newly-registered machine id. The server assigned it after
# /daemon/identify/; it is NOT the random hex in machine_id file.
log "querying /machines/ for new machine id"
MACHINES_JSON="$(curl -sS -H "Authorization: Bearer $VAST_HOST_API_KEY" \
    "$VAST_SERVER/api/v0/machines/")"
NEW_MID="$(printf '%s' "$MACHINES_JSON" | python3 -c 'import sys,json
d=json.load(sys.stdin)
m=d.get("machines") or []
if not m: sys.exit(0)
print(m[0].get("id"))')"

if [[ -z "$NEW_MID" ]]; then
    log "FAIL: no machine appeared under this account yet; check kaalia.log and rerun later"
    exit 2
fi
log "new machine id: $NEW_MID"

if [[ -n "$PRICE_GPU" ]]; then
    log "listing machine $NEW_MID for rent at \$${PRICE_GPU}/gpu/hr"
    LIST_RESP="$(curl -sS -X PUT \
        -H "Authorization: Bearer $VAST_HOST_API_KEY" \
        -H "Content-Type: application/json" \
        -d "{\"machine\": ${NEW_MID}, \"price_gpu\": ${PRICE_GPU}, \"price_disk\": ${PRICE_DISK}, \"price_inetu\": ${PRICE_INETU}, \"price_inetd\": ${PRICE_INETD}, \"min_chunk\": ${MIN_CHUNK}}" \
        "$VAST_SERVER/api/v0/machines/create_asks/")"
    log "create_asks response: $LIST_RESP"
    case "$LIST_RESP" in
        *'"success": true'*|*'"success":true'*)
            log "PASS: machine $NEW_MID listed"
            ;;
        *unauthenticated_machine*)
            log "FAIL: unauthenticated_machine still; support escalation required"
            exit 3
            ;;
        *)
            log "UNKNOWN: unexpected create_asks response"
            exit 4
            ;;
    esac
fi

log "done. machine $NEW_MID is registered; listing status above."
