# Break the one dependency cycle the installed release agent cannot repair
# itself: an interrupted handoff may leave Stado's exact Skarbiec proxy alive,
# with no recorded release owner and a dead candidate upstream. The host's
# last-good registry (or its physical canonical registry when no cache exists)
# supplies every coordinate below. This helper never guesses a bind, state
# path, plist, label, or readiness contract.
reconcile_skarbiec_bootstrap() {
  host=$(/bin/hostname -s | /usr/bin/tr '[:upper:]' '[:lower:]')
  release_registry="$HOME/.stado/cache/registry-last-good.json"
  if [ ! -r "$release_registry" ]; then
    release_registry="$store/registry.json"
  fi
  plan=$("$program" host local api skarbiec plan --registry "$release_registry" \
    --host "$host" --account "$account")
  IFS=$'\t' read -r managed target_name release_state proxy_state stable_bind \
    candidate_ports readiness_path legacy_plist legacy_label readiness_timeout <<< "$plan"
  if [ "$managed" = absent ]; then
    printf 'skarbiec_bootstrap unmanaged\n'
    return 0
  fi
  if [ "$managed" != managed ]; then
    printf 'skarbiec_bootstrap refused invalid_registry_plan\n' >&2
    return 1
  fi

  # Every wait below ends by the strategy's declared readiness wait
  # (release_control.products.skarbiec.strategy.readiness_timeout_seconds),
  # the only clock: each request may take what is left of it. remaining
  # prints nothing once it is spent, which curl refuses as a missing
  # --max-time, so a spent wait is a failed request, never curl's own
  # unlimited 0.
  deadline=$((SECONDS + readiness_timeout))
  remaining() {
    left=$((deadline - SECONDS))
    if [ "$left" -gt 0 ]; then printf '%s' "$left"; fi
  }
  ownership=$("$program" host local api skarbiec ownership --state "$release_state" \
    --target "$target_name")
  if [ "$ownership" = owned ]; then
    if /usr/bin/curl --silent --show-error --fail --max-time "$(remaining)" \
      "http://$stable_bind$readiness_path" >/dev/null 2>&1; then
      printf 'skarbiec_bootstrap active_release_owner stable=%s\n' "$stable_bind"
      return 0
    fi
    printf 'skarbiec bootstrap refused: release state owns an unavailable stable proxy\n' >&2
    return 1
  fi

  upstream=$("$program" host local api skarbiec upstream --state "$proxy_state" \
    --ports "$candidate_ports")
  if /usr/bin/curl --silent --show-error --fail --max-time "$(remaining)" \
    "http://$upstream$readiness_path" >/dev/null 2>&1; then
    printf 'skarbiec_bootstrap active_handoff upstream=%s\n' "$upstream"
    return 0
  fi

  processes="$work/skarbiec-release-proxies.txt"
  /bin/ps axww -o pid= -o command= > "$processes"
  match=$("$program" host local api skarbiec proxy --processes "$processes" \
    --state "$proxy_state" --bind "$stable_bind")
  IFS=$'\t' read -r match_kind proxy_pid proxy_executable <<< "$match"
  if [ "$match_kind" = none ]; then
    printf 'skarbiec_bootstrap no_exact_orphan\n'
    return 0
  fi
  if [ "$match_kind" != exact ] || [ -z "$proxy_pid" ] || [ -z "$proxy_executable" ]; then
    printf 'skarbiec_bootstrap refused invalid_process_match\n' >&2
    return 1
  fi
  expected_command="$proxy_executable release proxy --state $proxy_state --bind $stable_bind"
  observed_command=$(/bin/ps -p "$proxy_pid" -o command= 2>/dev/null || true)
  if [ "$observed_command" != "$expected_command" ]; then
    printf 'skarbiec_bootstrap refused proxy_changed_before_term\n' >&2
    return 1
  fi
  proxy_owner=$(
    /bin/ps -p "$proxy_pid" -o user= 2>/dev/null | /usr/bin/tr -d '[:space:]'
  )
  proxy_version=$("$proxy_executable" --version 2>/dev/null || true)
  if [ "$proxy_owner" != "$account" ] || [[ "$proxy_version" != stado\ * ]]; then
    printf 'skarbiec_bootstrap refused proxy_identity_mismatch\n' >&2
    return 1
  fi
  if [ "$legacy_plist" = "-" ] || [ "$legacy_label" = "-" ]; then
    if [ "$legacy_plist" != "-" ] || [ "$legacy_label" != "-" ]; then
      printf 'skarbiec_bootstrap refused partial_legacy_restore_plan\n' >&2
      return 1
    fi
    printf 'skarbiec_bootstrap refused exact_orphan_has_no_legacy_restore target=%s\n' \
      "$target_name" >&2
    return 1
  fi
  if [ ! -f "$legacy_plist" ]; then
    printf 'skarbiec_bootstrap refused legacy_plist_missing=%s\n' "$legacy_plist" >&2
    return 1
  fi
  plist_label=$(
    /usr/bin/plutil -extract Label raw -o - "$legacy_plist" 2>/dev/null || true
  )
  if [ "$plist_label" != "$legacy_label" ]; then
    printf 'skarbiec_bootstrap refused legacy_plist_label_mismatch\n' >&2
    return 1
  fi
  stable_port=${stable_bind##*:}
  listener_pid=$(
    /usr/sbin/lsof -nP -a -p "$proxy_pid" -iTCP:"$stable_port" \
      -sTCP:LISTEN -t 2>/dev/null | /usr/bin/sort -u || true
  )
  if [ "$listener_pid" != "$proxy_pid" ]; then
    printf 'skarbiec_bootstrap refused exact_proxy_does_not_own_bind\n' >&2
    return 1
  fi

  /bin/kill -TERM "$proxy_pid"
  while /bin/kill -0 "$proxy_pid" >/dev/null 2>&1; do
    if [ "$SECONDS" -ge "$deadline" ]; then
      printf 'skarbiec_bootstrap refused proxy_pid_did_not_exit pid=%s within readiness_timeout_seconds=%s\n' \
        "$proxy_pid" "$readiness_timeout" >&2
      return 1
    fi
    /bin/sleep 1
  done

  # Every restoration prerequisite was proved before the exact orphan was
  # signalled. From here the declared legacy unit is the only actor allowed to
  # reclaim the stable bind.
  set +e
  bootstrap_detail=$(
    /usr/bin/sudo -n /bin/launchctl bootstrap system "$legacy_plist" 2>&1
  )
  bootstrap_rc=$?
  set -e
  if [ "$bootstrap_rc" -ne 0 ] && [ "$bootstrap_rc" -ne 5 ]; then
    bootstrap_detail=$(printf '%s' "$bootstrap_detail" | /usr/bin/tr '\t\r\n' ' ')
    printf 'skarbiec_bootstrap refused bootstrap_%s:%s\n' \
      "$bootstrap_rc" "${bootstrap_detail:-launchctl said nothing}" >&2
    return 1
  fi
  /usr/bin/sudo -n /bin/launchctl enable "system/$legacy_label" >/dev/null 2>&1 || true
  while [ "$SECONDS" -lt "$deadline" ]; do
    if /usr/bin/sudo -n /bin/launchctl print "system/$legacy_label" >/dev/null 2>&1 &&
      /usr/bin/curl --silent --show-error --fail --max-time "$(remaining)" \
        "http://$stable_bind$readiness_path" >/dev/null 2>&1; then
      printf 'skarbiec_bootstrap restored target=%s bind=%s pid=%s\n' \
        "$target_name" "$stable_bind" "$proxy_pid"
      return 0
    fi
    /bin/sleep 1
  done
  printf 'skarbiec_bootstrap refused legacy_not_ready bind=%s\n' "$stable_bind" >&2
  return 1
}
