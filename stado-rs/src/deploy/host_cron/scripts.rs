//! The two remote programs, verbatim: the guarded prune and the guarded
//! restore, each with the substitution points this module spells for it.

/// Substitution point for the operator's pattern. Shell-quoted before it is
/// spliced, and never interpreted as a glob or a regex on the host: the match
/// is a literal substring test.
pub(super) const MATCH_MARK: &str = "@MATCH@";
/// Substitution point for `yes`/`no`.
pub(super) const APPLY_MARK: &str = "@APPLY@";

/// Read the table, judge one pattern against it, and — only with `apply=yes`
/// — install the table without that line.
pub(super) const PRUNE_SCRIPT: &str = r#"set -u
match=@MATCH@
apply=@APPLY@
report() { printf 'STADO_CRON\t%s\t%s\n' "$1" "$2"; }
b64() { /usr/bin/base64 | /usr/bin/tr -d '\n'; }
if ! /usr/bin/crontab -l >/dev/null 2>&1; then
  report absent "this account has no crontab"
  exit 0
fi
table=$(/usr/bin/crontab -l 2>/dev/null)
printf 'STADO_CRON_TABLE\t%s\n' "$(printf '%s\n' "$table" | b64)"
if [ -z "$match" ]; then
  report read "table returned, no pattern given"
  exit 0
fi
hits=$(printf '%s\n' "$table" | /usr/bin/grep -F -- "$match" | /usr/bin/grep -v '^[[:space:]]*#' || true)
count=$(printf '%s\n' "$hits" | /usr/bin/grep -c . || true)
if [ "$count" -eq 0 ]; then
  report absent "no crontab line contains that text"
  exit 0
fi
printf '%s\n' "$hits" | while IFS= read -r line; do
  [ -n "$line" ] || continue
  printf 'STADO_CRON_MATCH\t%s\n' "$(printf '%s' "$line" | b64)"
done
if [ "$count" -gt 1 ]; then
  report refused "$count lines contain that text; name one exactly - every matching line is printed above"
  exit 0
fi
# The fleet's own install root is the only thing that distinguishes an entry
# this product may prune from an entry that belongs to the machine's owner.
case "$hits" in
  *"$HOME/.stado/"*) ;;
  *) report refused "that line references nothing under \$HOME/.stado, so it is not this product's to remove"; exit 0 ;;
esac
if [ "$apply" != yes ]; then
  report read "one line matched and it is prunable; nothing was changed (pass --apply)"
  exit 0
fi
dir="$HOME/.stado/cron-backups"
/bin/mkdir -p "$dir" || { report refused "could not create $dir"; exit 0; }
stamp=$(/bin/date -u +%Y%m%dT%H%M%SZ)
backup="$dir/crontab-$stamp.bak"
printf '%s\n' "$table" > "$backup" || { report refused "could not write $backup"; exit 0; }
/bin/chmod 600 "$backup" 2>/dev/null || true
printf 'STADO_CRON_BACKUP\t%s\n' "$backup"
next="$dir/.crontab-next-$stamp"
printf '%s\n' "$table" | /usr/bin/grep -F -v -- "$match" > "$next" || true
if ! /usr/bin/crontab "$next"; then
  /bin/rm -f "$next"
  report refused "crontab refused the filtered table; the live table is unchanged and $backup holds it"
  exit 0
fi
/bin/rm -f "$next"
left=$(/usr/bin/crontab -l 2>/dev/null | /usr/bin/grep -F -c -- "$match" || true)
if [ "$left" != 0 ]; then
  report failed "the line is still in the installed table"
  exit 0
fi
report pruned "$backup"
"#;

/// Substitution point for a backup path on `--restore`.
pub(super) const RESTORE_MARK: &str = "@BACKUP@";

/// Install a table this command previously saved.
pub(super) const RESTORE_SCRIPT: &str = r#"set -u
backup=@BACKUP@
report() { printf 'STADO_CRON\t%s\t%s\n' "$1" "$2"; }
case "$backup" in
  "$HOME/.stado/cron-backups/"*) ;;
  *) report refused "only a table under \$HOME/.stado/cron-backups is restorable by this command"; exit 0 ;;
esac
if [ -L "$backup" ] || [ ! -f "$backup" ]; then
  report refused "no regular file at that path"
  exit 0
fi
if [ ! -O "$backup" ]; then
  report refused "that backup is not owned by this account"
  exit 0
fi
printf 'STADO_CRON_TABLE\t%s\n' "$(/usr/bin/base64 < "$backup" | /usr/bin/tr -d '\n')"
if ! /usr/bin/crontab "$backup"; then
  report refused "crontab refused that table; the live table is unchanged"
  exit 0
fi
report restored "$backup"
"#;
