    if any(character in field for character in "\t\r\n"):
        raise SystemExit("object API recovery refused: route contains control characters")
print("\t".join(field if field else "-" for field in fields))
PY
}

capture_loaded_route() {
  loaded=0
  loaded_backend=-
  loaded_primary_root=-
  loaded_backup_backend=-
  loaded_backup_root=-
  loaded_served_backend=-
  loaded_served_root=-
  loaded_legacy=-
  loaded_env_matches=-
  loaded_pid=-
  loaded_state=-
  loaded_explicit_backend=-
  loaded_explicit_root=-
  loaded_print="$work/$label.launchctl-print"
  if /usr/bin/sudo -n /bin/launchctl print "system/$label" \
    > "$loaded_print.tmp" 2>/dev/null; then
    /bin/mv "$loaded_print.tmp" "$loaded_print"
    /bin/chmod 600 "$loaded_print"
    runtime_state="$work/$label.runtime-state.json"
    if /usr/bin/curl --silent --show-error --fail --max-time 5 \
      "${object_url%/}/api/state.json" > "$runtime_state.tmp" 2>/dev/null; then
      /bin/mv "$runtime_state.tmp" "$runtime_state"
    else
      /bin/rm -f "$runtime_state.tmp" "$runtime_state"
    fi
    route=$(inspect_route launchctl "$loaded_print")
    IFS=$'\t' read -r loaded_backend loaded_primary_root \
      loaded_backup_backend loaded_backup_root loaded_served_backend \
      loaded_served_root loaded_legacy loaded_env_matches loaded_pid \
      loaded_state loaded_explicit_backend loaded_explicit_root <<< "$route"
    loaded=1
  else
    /bin/rm -f "$loaded_print.tmp"
  fi
}

loaded_ready_for_root() {
  expected_root=$1
  require_expected_environment=$2
  capture_loaded_route
  [ "$loaded" -eq 1 ] || return 1
  [ "$loaded_state" = running ] || return 1
  [[ "$loaded_pid" =~ ^[0-9]+$ ]] || return 1
  [ "$loaded_served_backend" = local ] || return 1
  [ "$loaded_served_root" = "$expected_root" ] || return 1
  [ "$loaded_legacy" = no ] || return 1
  if [ "$require_expected_environment" = yes ]; then
    [ "$loaded_env_matches" = yes ] || return 1
  fi
  listener_pids=$(
    /usr/bin/sudo -n /usr/sbin/lsof -nP -a -iTCP:8765 -sTCP:LISTEN -t \
      2>/dev/null | /usr/bin/sort -u || true
  )
  [ "$listener_pids" = "$loaded_pid" ] || return 1
  authenticated_object_ready
}

same=0
if /usr/bin/python3 - "$staged" "$plist" <<'PY'
import plistlib, sys
try:
    with open(sys.argv[1], "rb") as expected, open(sys.argv[2], "rb") as actual:
        same = plistlib.load(expected) == plistlib.load(actual)
except (OSError, plistlib.InvalidFileException):
    same = False
raise SystemExit(0 if same else 1)
PY
then same=1; fi

declared_backend=-
declared_primary_root=-
declared_backup_backend=-
declared_backup_root=-
declared_served_backend=-
declared_served_root=-
declared_legacy=-
declared_env_matches=-
declared_pid=-
declared_state=-
declared_explicit_backend=-
declared_explicit_root=-
if /usr/bin/sudo -n /bin/test -f "$plist"; then
  declared_route=$(inspect_route plist "$plist")
  IFS=$'\t' read -r declared_backend declared_primary_root \
    declared_backup_backend declared_backup_root declared_served_backend \
    declared_served_root declared_legacy declared_env_matches declared_pid \
    declared_state declared_explicit_backend declared_explicit_root \
    <<< "$declared_route"
fi

# Root movement is a different operation from listener recovery. In particular,
# copying a whole backup over the primary can restore stale queue and registry
# state. The resident transaction qualifies the namespace and captures writers;
# this helper must neither duplicate it nor re-enable a listener during it.
reconcile_skarbiec_bootstrap
capture_loaded_route
source_backend=local
source_root=$store
source_legacy=no
if [ "$loaded" -eq 1 ]; then
  source_backend=$loaded_served_backend
  source_root=$loaded_served_root
  source_legacy=$loaded_legacy
elif [ "$declared_served_backend" != "-" ]; then
  source_backend=$declared_served_backend
  source_root=$declared_served_root
  source_legacy=$declared_legacy
fi
if [ "$source_backend" != local ] || [ "$source_root" != "$store" ] ||
  [ "$source_legacy" != no ]; then
  printf 'storage_root_handoff_required backend=%s source=%s declared=%s; use stado host storage-root-reconcile for the authority transaction\n' \
    "$source_backend" "$source_root" "$store" >&2
  exit 70
fi

if loaded_ready_for_root "$store" yes; then
  if [ "$same" -eq 0 ]; then
    /usr/bin/sudo -n /usr/bin/install -m 644 -o root -g wheel "$staged" "$plist"
  fi
  printf 'already_healthy %s backend=local store=%s loaded_environment=matched\n' \
    "$label" "$store"
  exit 0
fi

healthy=0
if loaded_ready_for_root "$store" no; then healthy=1; fi
stamp=$(/bin/date -u +%Y%m%dT%H%M%SZ)
backup="$work/$label.plist.before-$stamp"
if /usr/bin/sudo -n /bin/test -f "$plist"; then
  /usr/bin/sudo -n /bin/cp "$plist" "$backup"
  /usr/bin/sudo -n /usr/sbin/chown "$account" "$backup"
  /bin/chmod 600 "$backup"
  printf 'backup %s\n' "$backup"
fi
if [ "$healthy" -eq 1 ]; then
  /usr/bin/sudo -n /usr/bin/install -m 644 -o root -g wheel "$staged" "$plist"
  printf 'persisted_while_healthy %s store=%s backup=%s loaded_job=unchanged\n' \
    "$label" "$store" "${backup:-none}"
  exit 0
fi

# Preserve the corrected definition before unload so an interrupted invocation
# can bootstrap the same root on its next run.
if [ "$same" -eq 1 ] && [ "$loaded" -eq 1 ]; then
  /usr/bin/sudo -n /bin/launchctl kickstart -k "system/$label"
  action=kickstarted
else
  /usr/bin/sudo -n /usr/bin/install -m 644 -o root -g wheel "$staged" "$plist"
  if [ "$loaded" -eq 1 ]; then
    /usr/bin/sudo -n /bin/launchctl bootout "system/$label"
  fi
  /usr/bin/sudo -n /bin/launchctl enable "system/$label" >/dev/null 2>&1 || true
  /usr/bin/sudo -n /bin/launchctl bootstrap system "$plist"
  action=reinstalled
fi

deadline=$((SECONDS + 180))
while [ "$SECONDS" -lt "$deadline" ]; do
  if loaded_ready_for_root "$store" yes; then
    printf '%s %s store=%s backup=%s\n' "$action" "$label" "$store" "${backup:-none}"
    exit 0
  fi
  /bin/sleep 1
done
printf 'authorization_timeout %s declared root did not serve authenticated reads after 180 seconds; backup=%s\n' \
  "$label" "${backup:-none}" >&2
exit 69
