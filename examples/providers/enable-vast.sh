#!/bin/sh
# enable-vast.sh — offer this fleet's idle GPU on the Vast.ai marketplace.
# The key comes from YOUR env: VAST_API_KEY (vast.ai console, account page).
# Usage: VAST_HOST=<host whose vault serves Skarbiec> PRICE_GPU=<usd/hour> \
#          PRICE_DISK=<usd/GB-month> sh enable-vast.sh
set -eu

: "${VAST_API_KEY:?VAST_API_KEY must hold the vast.ai API key}"
: "${VAST_HOST:?VAST_HOST must name the host whose vault serves Skarbiec}"
: "${PRICE_GPU:?PRICE_GPU must hold the per-GPU-hour price in USD}"
: "${PRICE_DISK:?PRICE_DISK must hold the per-GB-month disk price in USD}"

# 1. the key into the fleet vault as item `vast`, field `api_key`; it travels
#    on standard input, never as an argument
printf '{"api_key":"%s"}' "$VAST_API_KEY" |
  stado credentials item put --host "$VAST_HOST" --type api-key vast

# 2. confirm the marketplace accepts it
stado market readiness --provider vast

# 3. list the machine
stado market list --provider vast --price-gpu "$PRICE_GPU" --price-disk "$PRICE_DISK"
