//! The program that runs on the host, and the rendering of it for one unit
//! and one set of ports.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

use super::super::{MAX_OWNER_DEPTH, MAX_PORTS};

/// The remote program.
///
/// One launchd-domain scan is read once and reused for every owner walk: the
/// question "which job owns this pid" is asked per holder, and forking a
/// `launchctl` per holder would make the answer depend on how many processes
/// happened to hold the port. `launchctl list` sees only the caller's login
/// domain; the service manager on the always-on Mac owns jobs in `gui/<uid>`,
/// `user/<uid>`, and `system`. Reading the printable service tables is what
/// turns the pid that `service list --unowned` already proves is owned into the
/// exact label `service serving` needs.
const REMOTE_SERVING_BODY: &str = r##"
decode_flag=-D
if [ "$os" = "Linux" ]; then decode_flag=--decode; fi
ports_raw=$(printf '%s' '@PORTS_B64@' | /usr/bin/base64 "$decode_flag")

stado_launchd_state

# Read every printable launchd domain once. Each row is `pid<TAB>label`; a job
# whose pid is zero has no process to own and is deliberately omitted.
lc_table=''
if [ "$os" = "Darwin" ]; then
  uid=$(/usr/bin/id -u)
  for lc_domain in "gui/$uid" "user/$uid" system; do
    lc_rows=$(/bin/launchctl print "$lc_domain" 2>/dev/null | /usr/bin/awk '
      /services = \{/ { inside = 1; next }
      inside && /^[[:space:]]*\}/ { inside = 0 }
      inside && $1 ~ /^[1-9][0-9]*$/ && NF >= 3 {
        print $1 "\t" $3
      }
    ')
    lc_table="$lc_table${lc_table:+
}$lc_rows"
  done
fi

# The launchd job that owns a pid: the first pid on its own parent chain that
# one of the printable domains claims. Walking the chain is required, not
# defensive — a launcher script is the job and the server it starts is the
# child that holds the socket, so the listening pid is usually NOT the pid
# launchd recorded.
owner_label=''
owner_state=''
resolve_owner() {
  ro_pid="$1"
  ro_depth=0
  owner_label=''
  owner_state='unknown'
  while [ -n "$ro_pid" ] && [ "$ro_pid" != "1" ] && [ "$ro_depth" -lt @MAX_DEPTH@ ]; do
    ro_found=$(printf '%s\n' "$lc_table" | /usr/bin/awk -F'\t' -v P="$ro_pid" '$1 == P { print $2; exit }')
    if [ -n "$ro_found" ]; then
      owner_label="$ro_found"
      owner_state='resolved'
      return 0
    fi
    ro_pid=$(/bin/ps -p "$ro_pid" -o ppid= 2>/dev/null | /usr/bin/tr -d ' ')
    ro_depth=$((ro_depth + 1))
  done
  return 0
}

# Printable ASCII only, minus the two bytes a JSON string cannot carry raw.
# A label and a program name are host text and this report must not be
# breakable by either.
jsonsafe() {
  printf '%s' "$1" | /usr/bin/tr -c ' -~' '?' | /usr/bin/tr '"\\' '??'
}

listeners_state='read'
if ! /usr/sbin/lsof -nP -iTCP -sTCP:LISTEN >/dev/null 2>&1; then
  listeners_state='failed'
fi

ports_json=''
for port in $ports_raw; do
  case "$port" in ''|*[!0-9]*) continue ;; esac
  holders_json=''
  if [ "$listeners_state" = read ]; then
    for hpid in $(/usr/sbin/lsof -nP -tiTCP:"$port" -sTCP:LISTEN 2>/dev/null); do
      case "$hpid" in ''|*[!0-9]*) continue ;; esac
      hcomm=$(/bin/ps -p "$hpid" -o comm= 2>/dev/null | /usr/bin/sed 's/^ *//;s/ *$//')
      resolve_owner "$hpid"
      holders_json="$holders_json${holders_json:+,}{\"pid\":\"$hpid\",\"comm\":\"$(jsonsafe "$hcomm")\",\"owner\":\"$(jsonsafe "$owner_label")\",\"owner_state\":\"$owner_state\"}"
    done
  fi
  ports_json="$ports_json${ports_json:+,}{\"port\":$port,\"holders\":[$holders_json]}"
done

printf '{"unit":"%s","unit_path":"%s","loaded":"%s","launchd_pid":"%s","listeners_state":"%s","ports":[%s]}\n' \
  "$(jsonsafe "$unit")" "$(jsonsafe "$unit_path")" "$pc_loaded" "$(jsonsafe "$pc_pid")" \
  "$listeners_state" "$ports_json"
"##;

/// The remote program for one unit and one set of ports.
///
/// The ports travel base64-encoded inside the script body, never in an
/// argument vector, for the same reason every other reader here encodes its
/// operands.
pub fn remote_serving_script(ports: &[u16]) -> String {
    let list = ports
        .iter()
        .take(MAX_PORTS)
        .map(u16::to_string)
        .collect::<Vec<String>>()
        .join(" ");
    REMOTE_SERVING_BODY
        .replace("@PORTS_B64@", &STANDARD.encode(list.as_bytes()))
        .replace("@MAX_DEPTH@", &MAX_OWNER_DEPTH.to_string())
}
