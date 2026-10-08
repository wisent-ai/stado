#!/bin/sh
# enable-azure.sh — light up the Azure backend for your stado.
# Credentials come from YOUR env: AZURE_TENANT_ID, AZURE_CLIENT_ID, AZURE_CLIENT_SECRET,
# AZURE_ACCOUNT (the Microsoft account that signs in) and AZURE_OPERATOR_ROLE (the
# vault role that holds the operator session).
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

# 3. sign the operator in, then repair and verify the auth + RBAC contract
stado cloud login --provider azure --tenant "$AZURE_TENANT_ID" --account "$AZURE_ACCOUNT" \
  --role "$AZURE_OPERATOR_ROLE"
stado cloud roles repair --provider azure --operator-role "$AZURE_OPERATOR_ROLE"
