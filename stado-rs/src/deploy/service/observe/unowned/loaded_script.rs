/// Read-only: `launchctl list`, `launchctl print` for the three domains,
/// `launchctl dumpstate` once, and the unit files those name. It starts
/// nothing, stops nothing, signals nothing and needs no sudo.
///
/// Three reads, then one join, then one pass per label. The shape matters
/// because this script is what the fleet sweep spends its budget on. The
/// version before this one asked launchd again for every label -- five `awk`
/// passes over two in-memory tables and up to three `launchctl print
/// <domain>/<label>` calls each -- so a mac carrying 1,034 labels spawned
/// something near fifteen thousand processes to answer questions that one
/// pass answers for every label at once. It took longer than
/// [`crate::deploy::host_recovery::TIMEOUT_SECONDS`], the channel killed it,
/// and `stado doctor` reported `lukasz-macbook: not measured` -- the host
/// running the sweep was the one host the sweep could never finish. Measured
/// on that host: 28 seconds for 1,159 labels, against a 120-second cap it
/// used to exceed.
pub(crate) const LOADED_LABELS_SCRIPT: &str = r##"set -u
if [ "$(/usr/bin/uname -s)" != Darwin ]; then
  printf 'STADO_LOADED_UNSUPPORTED\t%s\n' "$(/usr/bin/uname -s)"
  exit 0
fi
uid=$(/usr/bin/id -u)
listing=$(/bin/launchctl list)

# Every job launchd actually HOLDS, in every domain this login can print.
#
# `launchctl list` prints one domain, and this script used to enumerate from it
# plus the three unit directories. A job loaded in the SYSTEM domain whose
# plist has been deleted is in neither half, so it was never a candidate. That
# is not a corner: on 2026-09-01 the label
# `com.wisent.compute.service.com.wisent.compute.service.stado-agent-mini` was
# loaded in the system domain with KeepAlive and no file on disk, and it
# recreated an undeclared `stado agent` on charless-mac-mini for days while
# `list --undeclared`, `list --unowned` and the reap keep-set each answered,
# for three different reasons, that no label held it.
holds=''
for domain in system "user/$uid" "gui/$uid"; do
  block=$(/bin/launchctl print "$domain" 2>/dev/null) || continue
  [ -n "$block" ] || continue
  rows=$(printf '%s\n' "$block" | /usr/bin/awk -v d="$domain" '
    /^[ \t]*services = \{/ { inside = 1; next }
    inside && /^[ \t]*\}/ { inside = 0 }
    inside {
      n = split($0, f, /[ \t]+/)
      while (n > 0 && f[n] == "") { n-- }
      if (n < 1) next
      lbl = f[n]
      if (lbl == "label" || lbl == "Label" || lbl == "PID") next
      p = (n >= 3 ? f[n - 2] : "")
      s = (n >= 2 ? f[n - 1] : "")
      printf "H\t%s\t%s\t%s\t%s\n", lbl, d, p, s
    }')
  holds="$holds
$rows"
done

# The unit file launchd loaded each job from, how many times it has started it,
# and how it last ended -- for every service in every domain, in one read.
#
# `launchctl dumpstate` states the whole world at once. The loop below used to
# ask `launchctl print <domain>/<label>` for these three columns, per label,
# per domain. The old comment here rejected dumpstate because it also carries
# every job's environment; that was an argument about what may LEAVE the host,
# and it is answered by extracting the three columns here rather than by
# spawning three thousand processes. Nothing but path, runs and last exit
# crosses the channel.
#
# A one-shot under KeepAlive reads `active` everywhere.
# `com.wisent.compute.service.com.wisent.claude-reauth-once` -- a job whose own
# name says `once` -- has run more than fifty thousand times, exiting 1 every
# time, into a log nobody read.
state=$(/bin/launchctl dumpstate 2>/dev/null | /usr/bin/awk '
  /^[^ \t].*= \{$/ { key = $1; next }
  /^\}/ { key = ""; next }
  key == "" { next }
  /^\tpath = / { sub(/^\tpath = /, ""); path[key] = $0; next }
  /^\truns = / { sub(/^\truns = /, ""); runs[key] = $0; next }
  /^\tlast exit code = / { sub(/^\tlast exit code = /, ""); exited[key] = $0; next }
  END {
    for (k in path) seen[k] = 1
    for (k in runs) seen[k] = 1
    for (k in exited) seen[k] = 1
    for (k in seen) {
      slash = 0
      for (i = length(k); i > 0; i--) { if (substr(k, i, 1) == "/") { slash = i; break } }
      if (slash < 2) continue
      printf "S\t%s\t%s\t%s\t%s\t%s\n", substr(k, slash + 1), substr(k, 1, slash - 1),
        (k in path ? path[k] : ""), (k in runs ? runs[k] : ""), (k in exited ? exited[k] : "")
    }
  }')

# Every label every source knows, joined once, in one `awk`.
joined=$(
  {
    printf '%s\n' "$listing" |
      /usr/bin/awk -F'\t' 'NF == 3 && $3 != "Label" { printf "L\t%s\t%s\t%s\n", $3, $1, $2 }'
    printf '%s\n' "$holds"
    printf '%s\n' "$state"
    for directory in /Library/LaunchDaemons "$HOME/Library/LaunchAgents" /Library/LaunchAgents; do
      for file in "$directory"/*.plist; do
        [ -f "$file" ] || continue
        base=${file##*/}
        printf 'F\t%s\t%s\n' "${base%.plist}" "$file"
      done
    done
  } | /usr/bin/awk -F'\t' '
    $1 == "L" { label[$2] = 1; lpid[$2] = $3; lstatus[$2] = $4; next }
    $1 == "H" {
      label[$2] = 1
      domains[$2] = domains[$2] $3 " "
      if (hpid[$2] == "") hpid[$2] = $4
      if (hstatus[$2] == "") hstatus[$2] = $5
      next
    }
    $1 == "S" {
      label[$2] = 1
      key = $2 SUBSEP $3
      spath[key] = $4; sruns[key] = $5; sexit[key] = $6
      order[$2] = order[$2] $3 " "
      next
    }
    $1 == "F" { label[$2] = 1; files[$2] = files[$2] $3 " "; next }
    END {
      for (l in label) {
        pid = lpid[l]; status = lstatus[l]
        # The domain table wins over the single-domain listing: it is the only
        # one of the two that can speak for the system domain.
        if (hpid[l] ~ /^[0-9]+$/) pid = hpid[l]
        if (status == "" && hstatus[l] ~ /^-?[0-9]+$/) status = hstatus[l]
        # A domain the job is loaded in answers first; any domain that knows
        # the label answers second. Both beat asking launchd again.
        chosen = ""
        n = split(domains[l], held, " ")
        for (i = 1; i <= n; i++) {
          if (held[i] != "" && (l SUBSEP held[i]) in spath) { chosen = held[i]; break }
        }
        if (chosen == "") {
          n = split(order[l], any, " ")
          for (i = 1; i <= n; i++) { if (any[i] != "") { chosen = any[i]; break } }
        }
        key = l SUBSEP chosen
        printf "J\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n", l, pid, status, domains[l],
          (key in spath ? spath[key] : ""), (key in sruns ? sruns[key] : ""),
          (key in sexit ? sexit[key] : ""), files[l]
      }
    }'
)

printf '%s\n' "$joined" | while IFS="$(printf '\t')" read -r tag label pid status loaded_domains launchd_path runs exited unit_files; do
  [ "$tag" = J ] || continue
  [ -n "$label" ] || continue
  case "$pid" in ''|*[!0-9]*) pid='' ;; esac
  case "$runs" in ''|*[!0-9]*) runs='' ;; esac
  case "$exited" in ''|*[!0-9-]*) exited='' ;; esac
  plist=''
  source=''
  domains=''
  for candidate in "/Library/LaunchDaemons/$label.plist" "$HOME/Library/LaunchAgents/$label.plist" "/Library/LaunchAgents/$label.plist"; do
    if [ -f "$candidate" ]; then
      if [ -z "$plist" ]; then plist="$candidate"; source='fleet-directory'; fi
      domains="${domains:+$domains }$candidate"
    fi
  done
  # A loaded label whose file is in none of the three directories this fleet
  # installs into. launchd knows where it loaded the job from, and the join
  # above already carries that answer.
  #
  # Only an absolute path is accepted. For a label with no unit file at all
  # -- every `application.com.apple.*` row on a mac -- launchd answers
  # `path = (submitted by runningboardd.190)`, and reporting that in a
  # UNIT_FILE column is a sentence that reads like a location and is not one.
  if [ -z "$plist" ]; then
    case "$launchd_path" in
      /*) plist="$launchd_path"; source='launchd' ;;
    esac
  fi
  program=''
  env_keys=''
  if [ -n "$plist" ] && [ -r "$plist" ]; then
    program=$(/usr/bin/plutil -extract ProgramArguments json -o - "$plist" 2>/dev/null \
      | /usr/bin/tr -d '[]"' | /usr/bin/tr ',' ' ' | /usr/bin/tr '\t\r\n' ' ')
    # The variables the unit file hands its program. A launchd job inherits
    # almost nothing, so a plist that names none of what its program requires
    # is a unit that cannot work -- and it fails on its interval, quietly,
    # forever. The beacon relay on lukasz-macbook carried HOME and PATH while
    # its program required STADO_HOST_HEALTH_API_URL, and it failed every five
    # minutes for three weeks.
    env_keys=$(/usr/bin/plutil -extract EnvironmentVariables json -o - "$plist" 2>/dev/null \
      | /usr/bin/tr -d '{}"' | /usr/bin/tr ',' '\n' \
      | /usr/bin/awk -F':' 'NF > 1 { print $1 }' | /usr/bin/tr '\n' ' ')
  fi
  # Which uppercase variables the program's own script reads, and which it sets
  # or defaults for itself. The subtraction happens in the reader, against the
  # set launchd does provide, because a shell is the wrong place to hold that
  # policy.
  needs=''
  assigns=''
  # `plutil -extract ... json` renders every path separator escaped
  # (`\/Users\/charles\/...`), which every earlier reader undid in Rust. This
  # one has to OPEN the file, so it unescapes here or it opens nothing -- and
  # opening nothing is how this check measured zero subjects on its first run
  # while looking, from the outside, exactly like a host with no problem.
  unescaped=$(printf '%s' "$program" | /usr/bin/tr -d '\\')
  script=$(printf '%s' "$unescaped" | /usr/bin/awk '{ print $1 }')
  case "$script" in
    */bash|*/sh|*/zsh|*/env) script=$(printf '%s' "$unescaped" | /usr/bin/awk '{ print $2 }') ;;
  esac
  case "$script" in
    "$HOME"/*)
      if [ -f "$script" ] && [ -r "$script" ]; then
        # `-I`, because the guard above is a path prefix and a path prefix does
        # not make a file a script. The old comment here promised "a fleet
        # script, never a system binary" and nothing enforced it:
        # `$HOME/.stado/bin/skarbiec`, `$HOME/.stado/marketplace/bin/wisent-agent`
        # and `$HOME/Applications/Oko.app/Contents/MacOS/Oko` are Mach-O
        # binaries of tens of megabytes. Grepping them cost most of this
        # script's runtime and answered `Binary file ... matches`, which the
        # reader then printed as the list of variables the program reads. A
        # binary states nothing about its environment here, and stating
        # nothing is the honest answer for it.
        needs=$(/usr/bin/grep -I -o -E '[$][{]?[A-Z][A-Z0-9_]{2,}' "$script" 2>/dev/null \
          | /usr/bin/tr -d '${' | /usr/bin/sort -u | /usr/bin/tr '\n' ' ')
        assigns=$(/usr/bin/grep -I -o -E '(^|[ \t;])(export[ ]+)?[A-Z][A-Z0-9_]{2,}=|[$][{][A-Z][A-Z0-9_]{2,}:[-=?]' "$script" 2>/dev/null \
          | /usr/bin/tr -d '${}:-=?' | /usr/bin/sed 's/^[ \t;]*//; s/^export[ ]*//' \
          | /usr/bin/sort -u | /usr/bin/tr '\n' ' ')
      fi
      ;;
  esac
  running=''
  started=''
  written=''
  case "$pid" in
    ''|*[!0-9]*) ;;
    *)
      running=$(/bin/ps -p "$pid" -o command= 2>/dev/null | /usr/bin/tr '\t\r\n' ' ')
      lstart=$(/bin/ps -p "$pid" -o lstart= 2>/dev/null)
      started=$(/bin/date -j -f '%a %b %d %T %Y' "$lstart" +%s 2>/dev/null)
      binary=$(/bin/ps -p "$pid" -o comm= 2>/dev/null)
      if [ -f "$binary" ]; then written=$(/usr/bin/stat -f %m "$binary" 2>/dev/null); fi
      ;;
  esac
  printf 'STADO_LOADED\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$pid" "$status" "$label" "${plist:--}" "${program:--}" "${domains:--}" "${running:--}" "${started:--}" "${written:--}" "${source:--}" "${loaded_domains:--}" "${runs:--}" "${exited:--}" "${env_keys:--}" "${needs:--}" "${assigns:--}"
done
# Every place a shell on this host could find a `stado`, and which one the
# release channel delivered.
#
# `~/.cargo/bin/stado` at 0.7.34 shadowed a delivered 0.13.40 for a week, and
# 0.7.34 has no `--undeclared`, no `bootout` and no `reap`: every answer it
# gave was "this host is clean", not because the host was, but because that
# binary could not look.
#
# `command -v` alone is not the question. This program runs on the channel's
# non-interactive shell, whose PATH is not the login shell's -- on
# charless-mac-mini it resolved NOTHING, and the reader called that agreement
# with the delivered binary. So the concrete locations are probed by name, a
# stale copy in any of them is a finding, and a location that could not be read
# is reported as unread rather than as clean.
delivered="$HOME/.stado/bin/stado"
delivered_version=''
delivered_real=''
if [ -x "$delivered" ]; then
  delivered_version=$("$delivered" --version 2>/dev/null | /usr/bin/awk '{ print $2; exit }')
  delivered_real=$(/usr/bin/readlink -f "$delivered" 2>/dev/null || printf '%s' "$delivered")
fi
printf 'STADO_PATH_DELIVERED\t%s\t%s\t%s\n' "$delivered" "${delivered_version:--}" "${delivered_real:--}"
# The delivered path is itself a candidate. Without it a host that carries
# exactly one correct binary measured ZERO locations and the reader had nothing
# to compare -- honest, but useless, and indistinguishable from a host nobody
# looked at.
#
# `-L` as well as `-e`: a DANGLING symlink on a PATH directory is not nothing,
# it is a `stado` that a shell finds and cannot execute.
for candidate in "$delivered" "$(command -v stado 2>/dev/null || true)" "$HOME/.cargo/bin/stado" "$HOME/.local/bin/stado" /usr/local/bin/stado /opt/homebrew/bin/stado; do
  [ -n "$candidate" ] || continue
  if [ ! -e "$candidate" ] && [ ! -L "$candidate" ]; then continue; fi
  version=''
  real=$(/usr/bin/readlink -f "$candidate" 2>/dev/null || printf '%s' "$candidate")
  if [ -x "$candidate" ]; then
    version=$("$candidate" --version 2>/dev/null | /usr/bin/awk '{ print $2; exit }')
  fi
  printf 'STADO_PATH_CANDIDATE\t%s\t%s\t%s\n' "$candidate" "${version:--}" "${real:--}"
done
"##;
