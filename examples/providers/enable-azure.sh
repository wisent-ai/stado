#!/bin/sh
# enable-azure.sh — light up the Azure backend for your stado.
# Credentials come from YOUR env: AZURE_TENANT_ID, AZURE_CLIENT_ID, AZURE_CLIENT_SECRET,
# AZURE_ACCOUNT (the Microsoft account that signs in) and AZURE_OPERATOR_ROLE (the
# vault role that holds the operator session).
# Usage: sh enable-azure.sh
set -eu

SB=${SKARBIEC_BIN:-skarbiec}

# 1. billing service principal into YOUR skarbiec, tagged with the role Stado
#    reads it by (fields per the billing contract); the item id is yours to choose
jq -n --arg tenant "$AZURE_TENANT_ID" --arg client "$AZURE_CLIENT_ID" \
  --arg secret "$AZURE_CLIENT_SECRET" \
  '{tenant_id: $tenant, client_id: $client, client_secret: $secret}' |
  "$SB" set-json azure-billing-principal --type env --tags stado:role:azure-billing

# 2. enable the provider in the stado config
jq '.providers = ((.providers + ["azure"]) | unique) | .providers_disabled -= ["azure"]' \
  ~/.config/stado/config.json > ~/.config/stado/config.json.new
mv ~/.config/stado/config.json.new ~/.config/stado/config.json

# 3. sign the operator in, then repair and verify the auth + RBAC contract
stado cloud login --provider azure --tenant "$AZURE_TENANT_ID" --account "$AZURE_ACCOUNT" \
  --role "$AZURE_OPERATOR_ROLE"
stado cloud roles repair --provider azure --operator-role "$AZURE_OPERATOR_ROLE"
