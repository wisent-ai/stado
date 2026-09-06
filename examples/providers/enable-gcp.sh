#!/bin/sh
# enable-gcp.sh — light up the GCP backend for your stado.
# Credentials come from YOUR env: GCP_SERVICE_ACCOUNT_JSON (full service-account JSON).
# Usage: sh enable-gcp.sh
set -eu

SB=${SKARBIEC_BIN:-skarbiec}

# 1. service account into YOUR skarbiec (field per the scoped GCP identity contract)
"$SB" set stado-gcp --type env "service_account_json=$GCP_SERVICE_ACCOUNT_JSON"

# 2. enable the provider in the stado config
jq '.providers = ((.providers + ["gcp"]) | unique) | .providers_disabled -= ["gcp"]' \
  ~/.config/stado/config.json > ~/.config/stado/config.json.new
mv ~/.config/stado/config.json.new ~/.config/stado/config.json

# 3. verify with the doctor probes
stado doctor
