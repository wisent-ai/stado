#!/bin/sh
# enable-vast.sh — light up the vast.ai marketplace for your stado.
# The key comes from YOUR env: VAST_API_KEY (vast.ai console, account page).
# Usage: sh enable-vast.sh
set -eu

SB=${SKARBIEC_BIN:-skarbiec}

# 1. api key into YOUR skarbiec (field per the vast provider contract)
"$SB" set stado-vast --type env "api_key=$VAST_API_KEY"

# 2. enable the provider in the stado config
jq '.providers = ((.providers + ["vast"]) | unique) | .providers_disabled -= ["vast"]' \
  ~/.config/stado/config.json > ~/.config/stado/config.json.new
mv ~/.config/stado/config.json.new ~/.config/stado/config.json

# 3. verify with a marketplace listing
stado vast list
