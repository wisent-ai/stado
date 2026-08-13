#!/usr/bin/env bash
# Install the Vast.ai host daemon on this machine and register it.
# Run on the RTX PRO 6000 host (10.0.0.36). Requires root.
#
# Usage:
#   sudo VAST_HOST_API_KEY=... bash vast_install.sh
#
# After this completes the GPU is advertised on vast.ai. The wisent-agent
# coexists via agent/src/vast.rs which yields Wisent jobs when a Vast
# rental is active on the same host.

set -euo pipefail
set -x

if [[ $EUID -ne 0 ]]; then
    echo "run as root" >&2
    exit 1
fi

: "${VAST_HOST_API_KEY:?VAST_HOST_API_KEY env var is required}"

command -v nvidia-smi  >/dev/null || { echo "nvidia-smi missing" >&2; exit 1; }
command -v docker      >/dev/null || { echo "docker missing"     >&2; exit 1; }
command -v systemctl   >/dev/null || { echo "systemctl missing"  >&2; exit 1; }

curl -fsSL https://vast.ai/install -o /tmp/vast_install.py
python3 /tmp/vast_install.py \
    --api-key "$VAST_HOST_API_KEY" \
    --accept-terms

systemctl daemon-reload
systemctl enable  vastai
systemctl restart vastai

# Reserve 40 GB VRAM for Wisent on the 96 GB RTX PRO 6000 so paid Vast
# rentals cannot starve the internal job queue.
mkdir -p /etc/vastai
cat >/etc/vastai/gpu_reserve.json <<EOF
{
  "reserved_vram_gb_per_gpu": 40,
  "reserved_cpu_cores": 2,
  "reserved_system_ram_gb": 16
}
EOF
systemctl restart vastai

systemctl status vastai --no-pager --lines=20
echo
echo "Vast.ai host daemon installed. Listing should appear on vast.ai"
echo "within 5 minutes. wisent-agent will detect active Vast rentals"
echo "via agent/src/vast.rs and yield Wisent jobs accordingly."
