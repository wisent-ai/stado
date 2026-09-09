//! The fixed remote program with `@IDENTITY_WORDS@` / `@WC_WORDS@` /
//! `@AGENT_ROWS@` substitution points. Written with explicit escapes so
//! `\t` / `\r` / `\n` are real control characters while `\\t` / `\\n`
//! remain literal backslash sequences where the marker protocol requires
//! them.

pub(super) const REMOTE_SCRIPT_TEMPLATE: &str = "set -u
host=$(/bin/hostname -s 2>/dev/null | /usr/bin/tr '[:upper:]' '[:lower:]')
identity_ok=0
for expected in @IDENTITY_WORDS@; do
  short=\"${expected%.local}\"
  if [ \"$host\" = \"$expected\" ] || [ \"$host\" = \"$short\" ]; then identity_ok=1; fi
done
if [ \"$identity_ok\" -ne 1 ]; then
  printf 'STADO_RECOVER\\tidentity_mismatch\\t%s\\n' \"$host\"
  exit 64
fi
if [ \"$(/usr/bin/uname -s)\" != \"Darwin\" ]; then
  printf 'STADO_RECOVER\\tunsupported_os\\t%s\\n' \"$(/usr/bin/uname -s)\"
  exit 65
fi

disk_before=$(/bin/df -k / 2>/dev/null | /usr/bin/awk 'NR==2 {print $4}')
wc_bin=\"\"
for candidate in @WC_WORDS@; do
  if [ -x \"$candidate\" ]; then wc_bin=\"$candidate\"; break; fi
done
cleanup_status=\"unavailable\"
cleanup_json=\"\"
if [ -n \"$wc_bin\" ]; then
  cleanup_json=$(\"$wc_bin\" disk-cleanup --once)
  cleanup_rc=$?
  if [ \"$cleanup_rc\" -eq 0 ]; then cleanup_status=\"ok\"; else cleanup_status=\"failed:$cleanup_rc\"; fi
fi

uid=$(/usr/bin/id -u)
gui=\"gui/$uid\"
user_domain=\"user/$uid\"
@DOMAIN_RESOLVER@
# The per-login domain this pass has, resolved for a LaunchAgent path by the
# same function every `stado service` verb resolves with, so a pass and a
# command cannot disagree about where a user agent lives. `launchd_domain` in
# the report is this answer, and it now carries why.
if stado_domain_of \"$HOME/Library/LaunchAgents\"; then
  domain_reason=$(printf '%s' \"$domain_reason\" | /usr/bin/tr '\t\r\n' ' ' | /usr/bin/cut -c1-160)
  printf 'STADO_DOMAIN\\t%s\\t%s\\t%s\\n' \"$domain\" \"$domain_status\" \"$domain_reason\"
else
  domain_reason=$(printf '%s' \"$domain_reason\" | /usr/bin/tr '\t\r\n' ' ' | /usr/bin/cut -c1-160)
  printf 'STADO_DOMAIN\\t%s\\t%s\\t%s\\n' \"$gui\" \"$domain_status\" \"$domain_reason\"
  exit 66
fi
/bin/launchctl bootout \"$gui/com.wisent.compute.coordinator\" >/dev/null 2>&1 || true
/bin/launchctl bootout \"$user_domain/com.wisent.compute.coordinator\" >/dev/null 2>&1 || true
/bin/launchctl disable \"$gui/com.wisent.compute.coordinator\" >/dev/null 2>&1 || true
/bin/launchctl disable \"$user_domain/com.wisent.compute.coordinator\" >/dev/null 2>&1 || true

recover_agent() {
  label=\"$1\"
  plist=\"$2\"
  if [ ! -f \"$plist\" ]; then
    printf 'STADO_AGENT\\t%s\\tmissing_plist\\n' \"$label\"
    return
  fi
  if [ \"$label\" = \"com.wisent.host-health-beacon\" ]; then
    api_url=$(/usr/bin/plutil -extract EnvironmentVariables.STADO_HOST_HEALTH_API_URL raw -o - \"$plist\" || true)
    vault_url=$(/usr/bin/plutil -extract EnvironmentVariables.STADO_HOST_HEALTH_SKARBIEC_URL raw -o - \"$plist\" || true)
    consumer=$(/usr/bin/plutil -extract EnvironmentVariables.STADO_HOST_HEALTH_SKARBIEC_CONSUMER raw -o - \"$plist\" || true)
    grant_file=$(/usr/bin/plutil -extract EnvironmentVariables.STADO_HOST_HEALTH_SKARBIEC_TOKEN_FILE raw -o - \"$plist\" || true)
    stado_bin=$(/usr/bin/plutil -extract EnvironmentVariables.STADO_BIN raw -o - \"$plist\" || true)
    if [ -z \"$api_url\" ] || [ -z \"$vault_url\" ] || [ -z \"$grant_file\" ] || [ -z \"$stado_bin\" ] || [ \"$consumer\" != \"stado-host-health-beacon\" ]; then
      printf 'STADO_AGENT\\t%s\\tinvalid_scoped_health_config\\n' \"$label\"
      return
    fi
    if /usr/bin/plutil -extract EnvironmentVariables.GOOGLE_APPLICATION_CREDENTIALS raw -o - \"$plist\" >/dev/null || /usr/bin/plutil -extract EnvironmentVariables.STADO_HOST_HEALTH_API_TOKEN raw -o - \"$plist\" >/dev/null; then
      printf 'STADO_AGENT\\t%s\\tforbidden_ambient_health_credential\\n' \"$label\"
      return
    fi
  fi
  # One resolver, per unit, and every verb below addresses what it chose: this
  # pass used to pick a domain once, bootstrap into it, and report `restarted`
  # on the bootstrap's exit status without ever asking launchd whether a job
  # was there.
  stado_domain_of \"$plist\" || true
  /bin/launchctl bootout \"$gui/$label\" >/dev/null 2>&1 || true
  /bin/launchctl bootout \"$user_domain/$label\" >/dev/null 2>&1 || true
  bootstrap_detail=$(/bin/launchctl bootstrap \"$domain\" \"$plist\" 2>&1)
  bootstrap_rc=$?
  /bin/launchctl enable \"$domain/$label\" >/dev/null 2>&1 || true
  /bin/launchctl kickstart -k \"$domain/$label\" >/dev/null 2>&1 || true
  if /bin/launchctl print \"$domain/$label\" >/dev/null 2>&1; then
    printf 'STADO_AGENT\t%s\trestarted\n' \"$label\"
    return
  fi
  bootstrap_detail=$(printf '%s' \"$bootstrap_detail\" | /usr/bin/tr '\t\r\n' ' ' | /usr/bin/cut -c1-160)
  if [ \"$bootstrap_rc\" -eq 0 ]; then
    printf 'STADO_AGENT\t%s\tnot_loaded:%s\n' \"$label\" \"${bootstrap_detail:-launchctl bootstrap said nothing and left no job}\"
  else
    printf 'STADO_AGENT\t%s\tbootstrap_failed:%s:%s\n' \"$label\" \"$bootstrap_rc\" \"$bootstrap_detail\"
  fi
}

# A unit the registry declares in launchd's system domain. This pass logs in
# as the approved unprivileged user, so `launchctl bootstrap system` is not
# available to it and the whole of `recover_agent` above would be a run of
# failures nobody sees, ending in a report of success. Look, say what is
# there, touch nothing: the caller turns these two words into the skipped
# entry and the blocker that make the overall status honest.
report_system_agent() {
  label=\"$1\"
  plist=\"$2\"
  if [ ! -f \"$plist\" ]; then
    printf 'STADO_AGENT\\t%s\\tmissing_plist\\n' \"$label\"
    return
  fi
  printf 'STADO_AGENT\\t%s\\tneeds_privileged_bootstrap\\n' \"$label\"
}

# The serving port every consumer's configuration names, and the one thing in
# this pass that takes root. It is guarded by the declaration and by a probe,
# in that order: a port that is already answering is left completely alone, so
# this can never collide with a live release-agent proxy. Only a declared
# stable bind that nothing holds gets its legacy daemon bootstrapped, which is
# the same `launchctl bootstrap system <plist>` the release agent's own
# `restore_legacy` runs when it hands the port back.
recover_stable_bind() {
  product=\"$1\"
  bind=\"$2\"
  plist=\"$3\"
  label=\"$4\"
  candidates=\"$5\"
  port=\"${bind##*:}\"
  if /usr/sbin/lsof -nP -iTCP:\"$port\" -sTCP:LISTEN >/dev/null 2>&1; then
    printf 'STADO_STABLE_BIND\\t%s\\t%s\\t%s\\n' \"$product\" \"$bind\" 'already_bound'
    return
  fi
  # A live candidate means a blue-green rollout is mid-flight or settled with
  # the legacy label AS the candidate: on this host, kickstarting that label
  # restarted the running Skarbiec (pid 15554 -> 83485) and moved nothing,
  # because the port it binds is the candidate. The actor that publishes a
  # stable bind is the release agent and only the release agent, so when a
  # candidate answers this stage says so and stops.
  for candidate in $candidates; do
    if /usr/sbin/lsof -nP -iTCP:\"$candidate\" -sTCP:LISTEN >/dev/null 2>&1; then
      printf 'STADO_STABLE_BIND\\t%s\\t%s\\tcandidate_live:%s\\n' \"$product\" \"$bind\" \"$candidate\"
      return
    fi
  done
  if [ ! -f \"$plist\" ]; then
    printf 'STADO_STABLE_BIND\\t%s\\t%s\\t%s\\n' \"$product\" \"$bind\" 'refused:missing_plist'
    return
  fi
  # bootstrap, enable, kickstart — the same order `recover_agent` uses above,
  # and every step is tried regardless of the previous one's status. launchd
  # answers `Bootstrap failed:
  # 5: Input/output error` both for a plist it cannot read and for a job it
  # already holds, and `release_agent`'s own `restore_legacy` treats that
  # status as success for exactly that reason. A job that is already
  # bootstrapped but not running is started by `kickstart -k` and by nothing
  # else, so a stage that stopped at the bootstrap status would report a
  # refusal for the case it can actually fix.
  detail=$(/usr/bin/sudo -n /bin/launchctl bootstrap system \"$plist\" 2>&1)
  rc=$?
  /usr/bin/sudo -n /bin/launchctl enable \"system/$label\" >/dev/null 2>&1 || true
  kick=$(/usr/bin/sudo -n /bin/launchctl kickstart -k \"system/$label\" 2>&1)
  kick_rc=$?
  if [ \"$rc\" -ne 0 ] && [ \"$kick_rc\" -eq 0 ]; then
    rc=0
    detail=\"$kick\"
  fi
  /bin/sleep 5
  if /usr/sbin/lsof -nP -iTCP:\"$port\" -sTCP:LISTEN >/dev/null 2>&1; then
    printf 'STADO_STABLE_BIND\\t%s\\t%s\\t%s\\n' \"$product\" \"$bind\" 'restored'
    return
  fi
  detail=$(printf '%s' \"$detail\" | /usr/bin/tr '\t\r\n' ' ' | /usr/bin/cut -c1-160)
  printf 'STADO_STABLE_BIND\\t%s\\t%s\\trefused:bootstrap_%s:%s\\n' \"$product\" \"$bind\" \"$rc\" \"${detail:-launchctl said nothing and the port is still unbound}\"
}

@STABLE_BIND_ROWS@

@AGENT_ROWS@
/bin/sleep 5
disk_after=$(/bin/df -k / 2>/dev/null | /usr/bin/awk 'NR==2 {print $4}')
printf 'STADO_RECOVER\\tok\\t%s\\t%s\\t%s\\t%s\\n' \"$host\" \"$disk_before\" \"$disk_after\" \"$cleanup_status\"
if [ -n \"$cleanup_json\" ]; then printf 'STADO_CLEANUP\\t%s\\n' \"$cleanup_json\"; fi
";
