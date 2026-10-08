#!/bin/sh
# enable-gcp.sh — light up the GCP backend for your stado.
# Credentials come from YOUR env: GCP_SERVICE_ACCOUNT_JSON (full service-account JSON).
# Usage: sh enable-gcp.sh
set -eu

SB=${SKARBIEC_BIN:-skarbiec}

# 1. service account into YOUR skarbiec, tagged with the role Stado reads it by
#    (field per the scoped GCP identity contract); the item id is yours to choose
jq -n --arg json "$GCP_SERVICE_ACCOUNT_JSON" '{service_account_json: $json}' |
  "$SB" set-json gcp-service-account --type env --tags stado:role:cloud-gcp

# 2. enable the provider in the stado config
jq '.providers = ((.providers + ["gcp"]) | unique) | .providers_disabled -= ["gcp"]' \
  ~/.config/stado/config.json > ~/.config/stado/config.json.new
mv ~/.config/stado/config.json.new ~/.config/stado/config.json

# 3. verify with the doctor probes
stado doctor
