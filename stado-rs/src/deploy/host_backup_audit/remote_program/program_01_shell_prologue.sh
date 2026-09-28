set -u
backup="$HOME/@BACKUP_ROOT@"
primary="$HOME/@PRIMARY_ROOT@"
program="$HOME/.stado/bin/stado"
if [ ! -d "$backup" ]; then
  printf 'STADO_BACKUP_AUDIT_UNAVAILABLE\t%s\n' 'local-backup physical root is absent'
  exit 0
fi
if [ ! -d "$primary" ]; then
  printf 'STADO_BACKUP_AUDIT_UNAVAILABLE\t%s\n' 'local-storage physical root is absent'
  exit 0
fi
if [ ! -x "$program" ]; then
  printf 'STADO_BACKUP_AUDIT_UNAVAILABLE\t%s\n' "this host has no installed Stado at $program to run the pass"
  exit 0
fi
# Free space as the host itself measures it, on both sides of the pass. The
# whole point of a reclaim is this number, so it is read by the program that
# changed it rather than by a second command an operator runs afterwards
# against a disk the fleet is still writing to.
free_kb() {
  /bin/df -Pk "$HOME" 2>/dev/null | /usr/bin/awk 'NR == 2 { print $4 }'
}
printf 'STADO_BACKUP_FREE\t%s\t%s\n' 'before' "$(free_kb)"
"$program" host backup-audit-local \
  --backup "$backup" \
  --primary "$primary" \
  --namespace '@NAMESPACE@' \
  --reclaim '@RECLAIM@' \
  --apply '@APPLY@' \
  --objects-hex '@OBJECTS_HEX@' \
  --inventory-namespaces-hex '@INVENTORY_NAMESPACES_HEX@'
