#!/bin/sh
# Where this host's agent stages job scratch, and who sets that.
#
# `disk_staging` redirects TMPDIR to the largest disk-backed mount only when
# TMPDIR is unset or /tmp; a TMPDIR already pointing somewhere else is kept
# verbatim, and the log line says so. On the RTX host it was kept pointing at
# `/mnt/wd16tb/wisent-staging` -- a removed disk, so the path resolves onto a
# 100 GiB root volume with 12 GiB free, which is exactly where multi-GB
# activation staging must not land.
#
# Read-only: the unit text, and only the staging-related assignments of its
# environment file.
set -eu

unit=wisent-agent.service

printf 'UNIT_TEXT\n'
systemctl cat "$unit" 2>&1 || true

printf '\nSTAGING_ASSIGNMENTS\n'
for file in /root/.stado/stado-agent.env /root/.stado/stado-agent-grant.env /etc/default/wisent-agent; do
  if [ -r "$file" ]; then
    matches=$(grep -nE '^(TMPDIR|TMP|TEMP|STADO_HF_FLUSH_STAGING_DIR|WISENT_[A-Z_]*STAGING[A-Z_]*)=' "$file" || true)
    if [ -n "$matches" ]; then
      printf '%s\n%s\n' "$file" "$matches"
    else
      printf '%s\tno staging assignment\n' "$file"
    fi
  fi
done

printf '\nTMPDIR_TARGET_STATE\n'
for path in /mnt/wd16tb/wisent-staging /mnt/wisent-staging; do
  if [ -d "$path" ]; then
    df -Ph "$path" | awk -v p="$path" 'NR==2 { print p "\t" $1 "\t" $4 " available" }'
  else
    printf '%s\tabsent\n' "$path"
  fi
done
