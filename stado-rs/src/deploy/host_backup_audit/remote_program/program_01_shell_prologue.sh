set -u
backup="$HOME/@BACKUP_ROOT@"
primary="$HOME/@PRIMARY_ROOT@"
if [ ! -d "$backup" ]; then
  printf 'STADO_BACKUP_AUDIT_UNAVAILABLE\t%s\n' 'local-backup physical root is absent'
  exit 0
fi
if [ ! -d "$primary" ]; then
  printf 'STADO_BACKUP_AUDIT_UNAVAILABLE\t%s\n' 'local-storage physical root is absent'
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
STADO_BACKUP_ROOT="$backup" \
STADO_PRIMARY_ROOT="$primary" \
STADO_NAMESPACE='@NAMESPACE@' \
STADO_HASH_DEADLINE='@HASH_DEADLINE@' \
STADO_RECLAIM='@RECLAIM@' \
STADO_APPLY='@APPLY@' \
STADO_OBJECTS_HEX='@OBJECTS_HEX@' \
STADO_INVENTORY_NAMESPACES_HEX='@INVENTORY_NAMESPACES_HEX@' \
/usr/bin/python3 - <<'STADO_AUDIT_EOF'
