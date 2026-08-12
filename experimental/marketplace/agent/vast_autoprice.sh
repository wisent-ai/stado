#!/usr/bin/env bash
# Feedback-driven pricing loop for a Vast.ai host machine.
#
# Model: the renter market reveals the right price. Offers that are currently
# rented (rented=true) are market-validated — renters paid that rate. We sit
# at the Nth percentile of confirmed-market prices for the same GPU class,
# then drift based on our own rental status:
#
#   - rented  -> nudge price up (we can capture more margin)
#   - idle    -> nudge price down (we need to attract renters), but never
#                below the cheapest comparable offer
#
# Usage (run as a user with a working `vast` CLI pointed at the host account):
#   MACHINE_ID=80858 GPU_NAME=RTX_PRO_6000_WS bash vast_autoprice.sh
#
# Intended to run via cron (every hour is enough — Vast's own bidding uses
# longer time scales). First invocation on a newly-listed machine seeds the
# price from market percentile; subsequent invocations tune up or down.
#
# Env:
#   MACHINE_ID      required; id of the machine to reprice
#   GPU_NAME        required; e.g. RTX_PRO_6000_WS (exact gpu_name value)
#   NUM_GPUS        default 1; number of GPUs in the offers to compare
#   SEED_PERCENTILE default 40; price percentile to anchor at on first seed
#   STEP_UP         default 0.05; multiplier to raise (1.05 = +5%)
#   STEP_DOWN       default 0.95; multiplier to lower (0.95 = -5%)
#   FLOOR_FACTOR    default 0.95; bootstrap floor as a fraction of cheapest
#                   comp. Used only until our own rental history file has
#                   entries; after that, floor = min observed rented price
#                   from $HISTORY_FILE.
#   HISTORY_FILE    default $HOME/vast_rental_history.csv; "ts,price" per row,
#                   appended whenever current_rentals_running > 0 at the
#                   listed_gpu_cost we currently advertise. The min-price
#                   entry is our true revenue floor.
#   PRICE_DISK      default 0.10; USD/GB/month
#   PRICE_INETU     default 0.01; USD/GB up (Vast caps around 0.04)
#   PRICE_INETD     default 0.01; USD/GB down (Vast caps around 0.04)
#   MIN_CHUNK       default 1

set -euo pipefail

: "${MACHINE_ID:?MACHINE_ID required}"
: "${GPU_NAME:?GPU_NAME required}"
NUM_GPUS="${NUM_GPUS:-1}"
SEED_PERCENTILE="${SEED_PERCENTILE:-40}"
STEP_UP="${STEP_UP:-1.05}"
STEP_DOWN="${STEP_DOWN:-0.95}"
FLOOR_FACTOR="${FLOOR_FACTOR:-0.95}"
HISTORY_FILE="${HISTORY_FILE:-$HOME/vast_rental_history.csv}"
PRICE_DISK="${PRICE_DISK:-0.10}"
PRICE_INETU="${PRICE_INETU:-0.01}"
PRICE_INETD="${PRICE_INETD:-0.01}"
MIN_CHUNK="${MIN_CHUNK:-1}"

command -v vast >/dev/null || { echo "vast CLI missing" >&2; exit 1; }
command -v jq   >/dev/null || { echo "jq missing"       >&2; exit 1; }

log() { printf '[%s] %s\n' "$(date -u +%FT%TZ)" "$*"; }

# --- Fetch comparable offers (same GPU + count, any verification state) ---
MARKET_JSON="$(vast search offers "gpu_name=$GPU_NAME num_gpus=$NUM_GPUS rentable=any verified=any" --raw 2>/dev/null)"

# Minimum price across all comparable offers (our hard floor * FLOOR_FACTOR).
MIN_ASK="$(printf '%s' "$MARKET_JSON" | jq '[.[] | select(.dph_base != null) | .dph_base] | if length > 0 then min else 0.40 end')"

# Subset: rented=true means a renter actually accepted that price.
RENTED_ASKS="$(printf '%s' "$MARKET_JSON" | jq '[.[] | select(.rented == true and .dph_base != null) | .dph_base] | sort')"
RENTED_COUNT="$(printf '%s' "$RENTED_ASKS" | jq 'length')"

# Percentile anchor on rented asks (first seed source of truth).
if [[ "$RENTED_COUNT" -gt 0 ]]; then
    SEED="$(printf '%s' "$RENTED_ASKS" | jq --argjson p "$SEED_PERCENTILE" '
      . as $a
      | (($p / 100) * (length - 1)) as $i
      | ($i | floor) as $lo
      | ($i | ceil) as $hi
      | if $lo == $hi then $a[$lo]
        else $a[$lo] + ($a[$hi] - $a[$lo]) * ($i - $lo)
        end')"
else
    # No confirmed-market anchor: fall back to overall median.
    SEED="$(printf '%s' "$MARKET_JSON" | jq '[.[] | select(.dph_base != null) | .dph_base] | sort | .[length/2|floor]')"
fi

# --- Fetch our machine state (listed, current rentals, current ask) ---
OUR_JSON="$(vast show machines --raw 2>/dev/null | jq --argjson id "$MACHINE_ID" '.machines[] | select(.id == $id)')"
LISTED="$(printf '%s' "$OUR_JSON" | jq '.listed')"
CUR_ASK="$(printf '%s' "$OUR_JSON" | jq '.listed_gpu_cost // 0')"
CUR_RUN="$(printf '%s' "$OUR_JSON" | jq '.current_rentals_running // 0')"

log "market: seed=\$${SEED} (p${SEED_PERCENTILE} of ${RENTED_COUNT} rented comps), min=\$${MIN_ASK}"
log "ours:   listed=${LISTED} cur_ask=\$${CUR_ASK} rentals_running=${CUR_RUN}"

# Journal our own rental history: if we have an active rental at the current
# listed price, that's a confirmed-rentable data point. Append with a
# timestamp; one row per hourly tick is fine.
if [[ "$CUR_RUN" -gt 0 ]] && [[ "$(echo "$CUR_ASK > 0" | bc -l)" == "1" ]]; then
    printf '%s,%s\n' "$(date -u +%FT%TZ)" "$CUR_ASK" >> "$HISTORY_FILE"
fi
# Real revenue floor from our history; fall back to market-based bootstrap.
HIST_MIN=""
if [[ -s "$HISTORY_FILE" ]]; then
    HIST_MIN="$(awk -F, '{print $2}' "$HISTORY_FILE" | sort -n | head -1)"
fi
if [[ -n "$HIST_MIN" ]]; then
    FLOOR="$HIST_MIN"
    FLOOR_SOURCE="history(min of $(wc -l < "$HISTORY_FILE") observed-rented points)"
else
    FLOOR="$(echo "$MIN_ASK * $FLOOR_FACTOR" | bc -l)"
    FLOOR_SOURCE="market_bootstrap(${FLOOR_FACTOR} * cheapest comp)"
fi
log "floor:  \$${FLOOR} (${FLOOR_SOURCE})"

# --- Decide the new price ---
if [[ "$LISTED" != "true" ]] || [[ "$(echo "$CUR_ASK == 0" | bc)" -eq 1 ]]; then
    NEW="$SEED"
    REASON="seed"
elif [[ "$CUR_RUN" -gt 0 ]]; then
    NEW="$(echo "$CUR_ASK * $STEP_UP" | bc -l)"
    REASON="raise (currently rented)"
else
    CANDIDATE="$(echo "$CUR_ASK * $STEP_DOWN" | bc -l)"
    UNDER="$(echo "$CANDIDATE < $FLOOR" | bc -l)"
    if [[ "$UNDER" == "1" ]]; then
        NEW="$FLOOR"
        REASON="lower (hit floor)"
    else
        NEW="$CANDIDATE"
        REASON="lower (idle)"
    fi
fi

# Round to 4 decimals to keep Vast happy.
NEW="$(printf '%.4f' "$NEW")"
log "decision: new_ask=\$${NEW} (${REASON})"

# --- Apply the new price ---
vast list machine "$MACHINE_ID" \
    --price_gpu "$NEW" \
    --price_disk "$PRICE_DISK" \
    --price_inetu "$PRICE_INETU" \
    --price_inetd "$PRICE_INETD" \
    --min_chunk "$MIN_CHUNK"
