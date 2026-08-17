#!/usr/bin/env bash
# Put the locally built `stado` over the installed one on this Linux host, or refuse.
#
# The same gate `install-built-stado-binary.py` applies on the Mac, for the same
# reason: the resolver, the host agent and every CLI call are this one
# executable.
#
#   - the candidate's version must not be older than the installed one;
#   - it must answer two read-only control-plane questions the way the installed
#     binary does, and where the installed binary cannot answer at all, the
#     candidate answering is recorded as a repair rather than a disagreement;
#   - the previous binary is kept beside the new one under its version and the
#     date, so one `cp` puts it back.
#
# The agent unit is restarted afterwards, because the agent is the point of the
# swap and its capacity broadcast is the evidence. Takes no operator words: a
# helper that took them would be a remote shell.
set -euo pipefail

BIN=/root/.stado/bin/stado
LOG=/root/.stado/build-work/stado-build.log
export PATH="/root/.cargo/bin:$PATH"
CANDIDATE=/root/.cache/stado-build/release/stado

[ -x "$CANDIDATE" ] || { printf 'ERROR\tno candidate at %s; run the build-stado helper\n' "$CANDIDATE" >&2; exit 1; }
if [ -f "$LOG" ] && ! grep -q '^BUILD_EXIT 0$' "$LOG"; then
  printf 'ERROR\tthe last build did not finish successfully; refusing to install\n' >&2
  tail -n 5 "$LOG" >&2 || true
  exit 1
fi

new_version=$("$CANDIDATE" --version | awk '{print $NF}')
old_version=$("$BIN" --version 2>/dev/null | awk '{print $NF}' || echo 0.0.0)
printf 'VERSION\tinstalled %s -> candidate %s\n' "$old_version" "$new_version"
older=$(printf '%s\n%s\n' "$new_version" "$old_version" | sort -V | head -n 1)
if [ "$older" = "$new_version" ] && [ "$new_version" != "$old_version" ]; then
  printf 'ERROR\tcandidate %s is older than installed %s\n' "$new_version" "$old_version" >&2
  exit 1
fi

for probe in "registry self" "registry pull"; do
  # shellcheck disable=SC2086
  if old_out=$("$BIN" $probe 2>/dev/null); then old_ok=1; else old_ok=0; old_out=""; fi
  # shellcheck disable=SC2086
  if new_out=$("$CANDIDATE" $probe 2>/dev/null); then new_ok=1; else new_ok=0; new_out=""; fi
  if [ "$new_ok" -eq 0 ]; then
    printf 'ERROR\tcandidate cannot answer `%s`; refusing the swap\n' "$probe" >&2
    exit 1
  fi
  if [ "$old_ok" -eq 1 ]; then
    if [ "$(printf '%s' "$old_out" | sha256sum)" = "$(printf '%s' "$new_out" | sha256sum)" ]; then
      printf 'PROBE\t%s\tidentical\n' "$probe"
    else
      printf 'ERROR\t%s differs between installed and candidate\n' "$probe" >&2
      exit 1
    fi
  else
    printf 'PROBE\t%s\trepair (installed binary could not answer)\n' "$probe"
  fi
done

backup="$BIN.$old_version-backup-$(date -u +%Y%m%d)"
cp -p "$BIN" "$backup"
install -m 0700 "$CANDIDATE" "$BIN"
printf 'INSTALL\t%s (previous kept at %s)\n' "$("$BIN" --version)" "$backup"

systemctl restart wisent-agent.service
sleep 20
printf 'UNIT\t%s\n' "$(systemctl is-active wisent-agent.service)"

printf '\nAGENT_LOG_TAIL\n'
journalctl -u wisent-agent.service --no-pager -n 12 -o cat | tail -n 12
