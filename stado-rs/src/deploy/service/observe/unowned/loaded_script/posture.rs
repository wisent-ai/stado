/// Every place a shell on this host could find a `stado`, and which one the
/// release channel delivered. Appended to [`super::LOADED_UNITS_SCRIPT`] for
/// the full read only; image reconciliation never asks this question.
///
/// `~/.cargo/bin/stado` at 0.7.34 shadowed a delivered 0.13.40 for a week, and
/// 0.7.34 has no `--undeclared`, no `bootout` and no `reap`: every answer it
/// gave was "this host is clean", not because the host was, but because that
/// binary could not look.
///
/// `command -v` alone is not the question. This program runs on the channel's
/// non-interactive shell, whose PATH is not the login shell's -- on
/// charless-mac-mini it resolved NOTHING, and the reader called that agreement
/// with the delivered binary. So the concrete locations are probed by name, a
/// stale copy in any of them is a finding, and a location that could not be
/// read is reported as unread rather than as clean.
pub(crate) const PATH_POSTURE_SCRIPT: &str = r##"delivered="$HOME/.stado/bin/stado"
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
