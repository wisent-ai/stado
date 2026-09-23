#!/bin/sh
# enable-aws.sh — light up the AWS backend for your stado.
# Credentials come from YOUR env: AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY
# (optional AWS_SESSION_TOKEN).
# Usage: sh enable-aws.sh
set -eu

SB=${SKARBIEC_BIN:-skarbiec}

# 1. credentials into YOUR skarbiec (fields per the AWS sdk contract)
"$SB" set stado-aws --type env \
  "access_key_id=$AWS_ACCESS_KEY_ID" \
  "secret_access_key=$AWS_SECRET_ACCESS_KEY"

# 2. enable the provider in the stado config
jq '.providers = ((.providers + ["aws"]) | unique) | .providers_disabled -= ["aws"]' \
  ~/.config/stado/config.json > ~/.config/stado/config.json.new
mv ~/.config/stado/config.json.new ~/.config/stado/config.json

# 3. verify the config holds
stado config validate
