#!/bin/sh
# enable-azure.sh — light up the Azure backend for your stado.
# Credentials come from YOUR env: AZURE_TENANT_ID, AZURE_CLIENT_ID, AZURE_CLIENT_SECRET.
# Usage: sh enable-azure.sh
set -eu

SB=${SKARBIEC_BIN:-skarbiec}

# 1. billing service principal into YOUR skarbiec (fields per the billing contract)
"$SB" set wisent-azure-billing-sp --type env \
  "tenant_id=$AZURE_TENANT_ID" \
  "client_id=$AZURE_CLIENT_ID" \
  "client_secret=$AZURE_CLIENT_SECRET"

# 2. enable the provider in the stado config
jq '.providers = ((.providers + ["azure"]) | unique) | .providers_disabled -= ["azure"]' \
  ~/.config/stado/config.json > ~/.config/stado/config.json.new
mv ~/.config/stado/config.json.new ~/.config/stado/config.json

# 3. verify the auth + RBAC contract
stado azure
