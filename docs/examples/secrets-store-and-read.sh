#!/bin/sh
# secrets-store-and-read.sh — the daily secrets loop.
# The value travels via stdin only (argv leaks into ps and history).
# Run: EXAMPLE_SECRET=... sh secrets-store-and-read.sh
set -eu

# store
printf '%s' "$EXAMPLE_SECRET" | stado secrets put demo-vendor

# read back
stado secrets get demo-vendor

# what this grant may see
stado secrets ls
