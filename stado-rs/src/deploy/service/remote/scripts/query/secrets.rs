/// Replace one assignment in an owner-only runtime environment file.
///
/// The secret rides inside the SSH request body as base64, never argv. The
/// remote shell decodes a complete shell-quoted assignment, removes prior
/// assignments of the same variable, and atomically renames a mode-600 file.
/// Existing unrelated variables stay on the host and never cross back to the
/// operator.
pub(crate) const SECRET_SYNC_BODY: &str = "fail_sync() {
  say 'secret_sync_failed' \"$1\"
  exit 0
}
if [ \"$os\" = \"Darwin\" ]; then decode_flag=-D; else decode_flag=--decode; fi
env_path=$(printf '%s' '@ENV_PATH_B64@' | /usr/bin/base64 \"$decode_flag\") || fail_sync 'invalid environment path payload'
case \"$env_path\" in
  \\$HOME/*) env_path=\"$HOME/${env_path#\\$HOME/}\" ;;
  /*) ;;
  *) fail_sync 'environment path is not rooted' ;;
esac
variable=@VARIABLE@
stado_sync_item=@ITEM@
stado_sync_field=@FIELD@
if [ -n \"$stado_sync_item\" ]; then
  value=$(\"$HOME/.stado/bin/stado\" credentials get \"$stado_sync_item\" --field \"$stado_sync_field\" 2>/dev/null) || fail_sync 'bearer unavailable on this host'
  [ -n \"$value\" ] || fail_sync 'bearer field is empty'
  export variable
  assignment=$(/usr/bin/env STADO_SYNC_VALUE=\"$value\" /usr/bin/python3 -c 'import os, shlex; print(os.environ[\"variable\"] + \"=\" + shlex.quote(os.environ[\"STADO_SYNC_VALUE\"]))' 2>/dev/null) || fail_sync 'cannot render the assignment'
else
  assignment=$(printf '%s' '@ASSIGNMENT_B64@' | /usr/bin/base64 \"$decode_flag\") || fail_sync 'invalid assignment payload'
fi
parent=$(/usr/bin/dirname \"$env_path\") || fail_sync 'environment parent unavailable'
/bin/mkdir -p \"$parent\" || fail_sync 'cannot create environment parent'
tmp=\"$env_path.stado-secret-sync.$$\"
trap '/bin/rm -f \"$tmp\"' EXIT HUP INT TERM
if [ -f \"$env_path\" ]; then
  /usr/bin/awk -v key=\"$variable\" '
    $0 ~ \"^[[:space:]]*(export[[:space:]]+)?\" key \"=\" { next }
    { print }
  ' \"$env_path\" > \"$tmp\" || fail_sync 'cannot filter environment file'
else
  : > \"$tmp\" || fail_sync 'cannot create environment file'
fi
printf '%s\\n' \"$assignment\" >> \"$tmp\" || fail_sync 'cannot append assignment'
/bin/chmod 600 \"$tmp\" || fail_sync 'cannot protect environment file'
/bin/mv -f \"$tmp\" \"$env_path\" || fail_sync 'cannot install environment file'
trap - EXIT HUP INT TERM
say 'secret_synced' \"$variable $env_path\"
";

/// Authenticate one read-only loopback request from the managed host.
///
/// The bearer is staged in an owner-only curl header file, never argv. Both
/// the header and response body are removed before the marker is emitted.
pub(crate) const AUTH_CHECK_BODY: &str = "stado_check_item=@ITEM@
stado_check_field=@FIELD@
stado_check_var=@VARIABLE@
stado_check_env_b64=@ENV_PATH_B64@
stado_check_consumer=@CONSUMER@
stado_check_token_file=@TOKEN_FILE@
fail_check() {
  say 'auth_check_failed' \"$1\"
  exit 0
}
if [ \"$os\" = \"Darwin\" ]; then decode_flag=-D; else decode_flag=--decode; fi
probe_url=$(printf '%s' '@PROBE_URL_B64@' | /usr/bin/base64 \"$decode_flag\") || fail_check 'invalid probe URL payload'
if [ -n \"$stado_check_consumer\" ]; then
  export WC_SKARBIEC_CONSUMER=\"$stado_check_consumer\"
fi
if [ -n \"$stado_check_token_file\" ]; then
  export WC_SKARBIEC_TOKEN_FILE=\"$stado_check_token_file\"
fi
probe_dir=\"$HOME/.stado/auth-check\"
/bin/mkdir -p \"$probe_dir\" || fail_check 'cannot create probe directory'
/bin/chmod 700 \"$probe_dir\" || fail_check 'cannot protect probe directory'
resolve_err=\"\"
resolved=\"\"
if [ -n \"$stado_check_item\" ]; then
  probe_log=\"$probe_dir/resolve.log\"
  : > \"$probe_log\"
  resolved=$(\"$HOME/.stado/bin/stado\" secrets get \"$stado_check_item\" --field \"$stado_check_field\" 2>\"$probe_log\")
  src=secrets-get
  # Source 2/3: legacy credential-store and direct Skarbiec reads.
  if [ -z \"$resolved\" ]; then
    resolved=$(\"$HOME/.stado/bin/stado\" credentials get \"$stado_check_item\" --field \"$stado_check_field\" 2>>\"$probe_log\") && src=credentials-get
  fi
  if [ -z \"$resolved\" ]; then
    resolved=$(\"$HOME/.stado/bin/skarbiec\" get \"$stado_check_item\" --field \"$stado_check_field\" 2>>\"$probe_log\") && src=skarbiec-get
  fi
  [ -n \"$resolved\" ] || { resolve_err=$(src=$src; /usr/bin/tail -c 300 \"$probe_log\" 2>/dev/null); fail_check \"bearer unavailable via $src${resolve_err:+: $resolve_err}\"; }
elif [ -n \"$stado_check_var\" ]; then
  check_env_path=$(printf '%s' '@ENV_PATH_B64@' | /usr/bin/base64 \"$decode_flag\") || fail_check 'invalid environment path payload'
  case \"$check_env_path\" in
    \\$HOME/*) check_env_path=\"$HOME/${check_env_path#\\$HOME/}\" ;;
    /*) ;;
    *) fail_check 'environment path is not rooted' ;;
  esac
  [ -f \"$check_env_path\" ] || fail_check 'runtime environment file is absent'
  resolved=$(/usr/bin/awk -F= -v key=\"$stado_check_var\" '$1 == key { v=substr($0, length($1)+2); gsub(/^[\"]+|[\"]+$/, \"\", v); print v }' \"$check_env_path\")
  [ -n \"$resolved\" ] || fail_check 'environment variable is empty or absent'
else
  resolved=$(printf '%s' '@TOKEN_B64@' | /usr/bin/base64 \"$decode_flag\") || fail_check 'invalid token payload'
fi
token=\"$resolved\"
[ -n \"$token\" ] || fail_check 'bearer field is empty'
post_empty=@POST_EMPTY@
expected_status=@EXPECTED_STATUS@
probe_dir=\"$HOME/.stado/auth-check\"
/bin/mkdir -p \"$probe_dir\" || fail_check 'cannot create probe directory'
/bin/chmod 700 \"$probe_dir\" || fail_check 'cannot protect probe directory'
header=\"$probe_dir/header.$$\"
response=\"$probe_dir/response.$$\"
error_file=\"$probe_dir/error.$$\"
trap '/bin/rm -f \"$header\" \"$response\" \"$error_file\"' EXIT HUP INT TERM
printf 'Authorization: Bearer %s\\n' \"$token\" > \"$header\" || fail_check 'cannot stage authorization header'
unset token
/bin/chmod 600 \"$header\" || fail_check 'cannot protect authorization header'
if [ \"$post_empty\" = yes ]; then
  status=$(/usr/bin/curl --silent --show-error --max-time 15 --output \"$response\" --write-out '%{http_code}' --request POST --header 'Content-Type: application/json' --header \"@$header\" --data '{}' \"$probe_url\" 2>\"$error_file\")
  rc=$?
else
  status=$(/usr/bin/curl --silent --show-error --max-time 15 --output \"$response\" --write-out '%{http_code}' --header \"@$header\" \"$probe_url\" 2>\"$error_file\")
  rc=$?
fi
/bin/rm -f \"$header\" \"$response\" \"$error_file\"
trap - EXIT HUP INT TERM
if [ \"$rc\" -ne 0 ]; then
  say 'auth_unreachable' \"curl exit $rc\"
elif [ -n \"$expected_status\" ] && [ \"$status\" = \"$expected_status\" ]; then
  say 'auth_ok' \"HTTP $status\"
elif [ -z \"$expected_status\" ] && [ \"$status\" -ge 200 ] 2>/dev/null && [ \"$status\" -lt 300 ] 2>/dev/null; then
  say 'auth_ok' \"HTTP $status\"
elif [ \"$status\" = 401 ] || [ \"$status\" = 403 ]; then
  say 'auth_rejected' \"HTTP $status\"
else
  say 'auth_failed' \"HTTP $status\"
fi
";

/// Stop the process currently owning a checked loopback port.
///
/// This is deliberately Darwin-only and separate from ordinary restart:
/// launchd cannot replace an unmanaged fallback process that still owns the
/// service port.
pub(crate) const LISTENER_RESET_BODY: &str = "if [ \"$os\" != \"Darwin\" ]; then
  say 'listener_reset_unsupported' \"$os\"
  exit 0
fi
port=@PORT@
pids=$(/usr/sbin/lsof -nP -tiTCP:\"$port\" -sTCP:LISTEN 2>/dev/null)
if [ -z \"$pids\" ]; then
  say 'listener_absent' \"$port\"
  exit 0
fi
listener_detail=\"$port\"
for pid in $pids; do
  case \"$pid\" in *[!0-9]*) say 'listener_reset_failed' 'invalid pid'; exit 0 ;; esac
  owner=$(/bin/ps -p \"$pid\" -o ppid=,comm= 2>/dev/null | /usr/bin/tr '\t\r\n' ' ')
  listener_detail=\"$listener_detail pid=$pid $owner\"
  /bin/kill -TERM \"$pid\" >/dev/null 2>&1 || true
done
/bin/sleep 1
for pid in $pids; do
  if /bin/kill -0 \"$pid\" >/dev/null 2>&1; then
    /bin/kill -KILL \"$pid\" >/dev/null 2>&1 || true
  fi
done
say 'listener_stopped' \"$listener_detail\"
";
