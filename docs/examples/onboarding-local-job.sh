#!/bin/sh
# onboarding-local-job.sh — from zero to one completed local job.
# No cloud account needed. Run: sh onboarding-local-job.sh
set -eu

stado config init
stado config validate
stado doctor --fix-hints

# submit a trivial local workload and watch it finish
JOB_ID=$(stado submit --profile local -- echo hello-from-stado | awk -F'"' '/"id"/ {print $4; exit}')
echo "submitted: $JOB_ID"
stado job watch "$JOB_ID"

# the result, downloaded; results takes JOB_ID and OUTPUT_DIR
OUT_DIR=$(mktemp -d)
stado results "$JOB_ID" "$OUT_DIR"
ls -la "$OUT_DIR"
