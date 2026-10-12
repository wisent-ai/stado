pruned=0
# The apply mark is Rust's `true` or `false`, already false unless reclaim was asked.
if [ '@APPLY@' = true ]; then
  # Counted as the difference the delete made rather than as the empty
  # directories seen beforehand: `-delete` empties parents as it descends.
  before=$(/usr/bin/find "$backup" -type d 2>/dev/null | /usr/bin/wc -l | /usr/bin/tr -d ' ')
  /usr/bin/find "$backup" -mindepth 1 -type d -empty -delete 2>/dev/null
  after=$(/usr/bin/find "$backup" -type d 2>/dev/null | /usr/bin/wc -l | /usr/bin/tr -d ' ')
  pruned=$((before - after))
fi
printf 'STADO_BACKUP_PRUNED\t%s\n' "$pruned"
printf 'STADO_BACKUP_FREE\t%s\t%s\n' 'after' "$(free_kb)"
