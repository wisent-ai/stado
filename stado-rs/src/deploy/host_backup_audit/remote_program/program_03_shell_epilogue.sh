STADO_AUDIT_EOF
pruned=0
if [ '@APPLY@' = yes ] && [ '@RECLAIM@' = yes ]; then
  # Counted as the difference the delete made rather than as the empty
  # directories seen beforehand: `-delete` empties parents as it descends.
  before=$(/usr/bin/find "$backup" -type d 2>/dev/null | /usr/bin/wc -l | /usr/bin/tr -d ' ')
  /usr/bin/find "$backup" -mindepth 1 -type d -empty -delete 2>/dev/null
  after=$(/usr/bin/find "$backup" -type d 2>/dev/null | /usr/bin/wc -l | /usr/bin/tr -d ' ')
  pruned=$((before - after))
fi
printf 'STADO_BACKUP_PRUNED\t%s\n' "$pruned"
printf 'STADO_BACKUP_FREE\t%s\t%s\n' 'after' "$(free_kb)"
