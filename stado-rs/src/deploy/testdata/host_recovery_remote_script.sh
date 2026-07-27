set -u
host=$(/bin/hostname -s 2>/dev/null | /usr/bin/tr '[:upper:]' '[:lower:]')
identity_ok=0
for expected in mini-one mini-one.lan mini-one.local; do
  short="${expected%.local}"
  if [ "$host" = "$expected" ] || [ "$host" = "$short" ]; then identity_ok=1; fi
done
if [ "$identity_ok" -ne 1 ]; then
  printf 'STADO_RECOVER\tidentity_mismatch\t%s\n' "$host"
  exit 64
fi
if [ "$(/usr/bin/uname -s)" != "Darwin" ]; then
  printf 'STADO_RECOVER\tunsupported_os\t%s\n' "$(/usr/bin/uname -s)"
  exit 65
fi

disk_before=$(/bin/df -k / 2>/dev/null | /usr/bin/awk 'NR==2 {print $4}')
wc_bin=""
for candidate in "$HOME/.venvs/wisent-compute/bin/wc" "$HOME/.local/bin/wc" "/opt/homebrew/bin/wc"; do
  if [ -x "$candidate" ]; then wc_bin="$candidate"; break; fi
done
cleanup_status="unavailable"
cleanup_json=""
if [ -n "$wc_bin" ]; then
  cleanup_json=$(GOOGLE_APPLICATION_CREDENTIALS="${GOOGLE_APPLICATION_CREDENTIALS:-$HOME/.config/gcloud/application_default_credentials.json}" "$wc_bin" disk-cleanup --once 2>/dev/null)
  cleanup_rc=$?
  if [ "$cleanup_rc" -eq 0 ]; then cleanup_status="ok"; else cleanup_status="failed:$cleanup_rc"; fi
fi

uid=$(/usr/bin/id -u)
gui="gui/$uid"
user_domain="user/$uid"
if /bin/launchctl print "$gui" >/dev/null 2>&1; then
  agent_domain="$gui"
  printf 'STADO_DOMAIN	%s	available
' "$agent_domain"
elif /bin/launchctl print "$user_domain" >/dev/null 2>&1; then
  agent_domain="$user_domain"
  printf 'STADO_DOMAIN	%s	fallback
' "$agent_domain"
else
  printf 'STADO_DOMAIN	%s	unavailable
' "$gui"
  exit 66
fi
/bin/launchctl bootout "$gui/com.wisent.compute.coordinator" >/dev/null 2>&1 || true
/bin/launchctl bootout "$user_domain/com.wisent.compute.coordinator" >/dev/null 2>&1 || true
/bin/launchctl disable "$gui/com.wisent.compute.coordinator" >/dev/null 2>&1 || true
/bin/launchctl disable "$user_domain/com.wisent.compute.coordinator" >/dev/null 2>&1 || true

recover_agent() {
  label="$1"
  plist="$2"
  if [ ! -f "$plist" ]; then
    printf 'STADO_AGENT\t%s\tmissing_plist\n' "$label"
    return
  fi
  /bin/launchctl bootout "$gui/$label" >/dev/null 2>&1 || true
  /bin/launchctl bootout "$user_domain/$label" >/dev/null 2>&1 || true
  bootstrap_detail=$(/bin/launchctl bootstrap "$agent_domain" "$plist" 2>&1)
  bootstrap_rc=$?
  if [ "$bootstrap_rc" -eq 0 ]; then
    /bin/launchctl enable "$agent_domain/$label" >/dev/null 2>&1 || true
    /bin/launchctl kickstart -k "$agent_domain/$label" >/dev/null 2>&1 || true
    printf 'STADO_AGENT	%s	restarted
' "$label"
  else
    bootstrap_detail=$(printf '%s' "$bootstrap_detail" | /usr/bin/tr '	
' ' ' | /usr/bin/cut -c1-160)
    printf 'STADO_AGENT	%s	bootstrap_failed:%s:%s
' "$label" "$bootstrap_rc" "$bootstrap_detail"
  fi
}

recover_agent com.wisent.compute.auto-deployer "$HOME/Library/LaunchAgents/com.wisent.compute.auto-deployer.plist"
recover_agent com.wisent.weles-auto-deploy "$HOME/Library/LaunchAgents/com.wisent.weles-auto-deploy.plist"
recover_agent com.wisent.weles-worker "$HOME/Library/LaunchAgents/com.wisent.weles-worker.plist"
recover_agent com.wisent.weles-keyword-planner-api "$HOME/Library/LaunchAgents/com.wisent.weles-keyword-planner-api.plist"
recover_agent com.wisent.host-health-beacon "$HOME/Library/LaunchAgents/com.wisent.host-health-beacon.plist"
/bin/sleep 5
disk_after=$(/bin/df -k / 2>/dev/null | /usr/bin/awk 'NR==2 {print $4}')
printf 'STADO_RECOVER\tok\t%s\t%s\t%s\t%s\n' "$host" "${disk_before:-0}" "${disk_after:-0}" "$cleanup_status"
if [ -n "$cleanup_json" ]; then printf 'STADO_CLEANUP\t%s\n' "$cleanup_json"; fi
