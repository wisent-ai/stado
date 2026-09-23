#!/usr/bin/env bash
# Watch one job exclusively through Stado's configured control boundary.
# The command exits with the terminal job outcome and follows the canonical log
# without provider credentials, bucket names, GitHub side effects, or ADC.
#
# Usage: watch_job.sh <job_id>

set -euo pipefail
JOB="${1:?job_id required}"
STADO_BIN="${STADO_BIN:-stado}"

exec "$STADO_BIN" job watch "$JOB" --follow
