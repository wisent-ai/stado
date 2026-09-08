# Skarbiec vault inventory: METADATA ONLY.
#
# "Which Skarbiec vaults are on this host" is answerable from stat(2), so it
# is answered from stat(2). Nothing below opens a vault, reads a byte of
# ciphertext, counts items, or names a consumer — a vault is a file of
# secrets, and the inventory says one exists and how big it is, never what
# is in it. There is no field in this section that could carry content.
emit_vault_file() {
  vault_path="$1"
  # basename(1) by parameter expansion, for the same reason as the markers.
  vault_name=${vault_path##*/}
  # -L first, and never -L then -f, for the same reason as the binaries: -f
  # follows the link, so a symlink would be reported as a present vault and
  # its target's metadata read instead of the link's.
  if [ -L "$vault_path" ]; then
    vault_state=refused_symlink
  elif [ -f "$vault_path" ]; then
    vault_state=regular
  else
    vault_state=refused_not_regular
  fi
  # Both stat(1) dialects lstat by default, so a symlink reports the link's
  # own size and mode and its target is never touched. BSD form first, GNU
  # form second; neither opens the file.
  if vault_facts=$(/usr/bin/stat -f '%z %Lp' "$vault_path" 2>/dev/null); then
    :
  elif vault_facts=$(/usr/bin/stat -c '%s %a' "$vault_path" 2>/dev/null); then
    :
  else
    vault_facts=""
  fi
  # Split the two fields with parameter expansion instead of two awk forks.
  # bytes is a JSON integer, so a stat that did not answer has to become 0
  # rather than an empty token that breaks the payload, and a mode that did
  # not arrive has to say "unknown" rather than arrive blank.
  vault_bytes=${vault_facts%% *}
  vault_mode=${vault_facts#* }
  case "$vault_bytes" in
    ''|*[!0-9]*) vault_bytes=0 ;;
  esac
  if [ "$vault_mode" = "$vault_facts" ]; then
    vault_mode=unknown
  fi
  # Normalize the two dialects onto one spelling: 0600 and 600 are the same
  # mode, and owner_only must not depend on which stat answered.
  case "$vault_mode" in
    0???) vault_mode=${vault_mode#0} ;;
  esac
  case "$vault_mode" in
    *[!0-7]*) vault_mode=unknown ;;
  esac
  case "$vault_mode" in
    *00) vault_owner_only=true ;;
    *) vault_owner_only=false ;;
  esac
  sanitize "$vault_name"
  vault_name_safe=$sanitized
  sanitize "$vault_mode"
  printf '%s{"name":"%s","state":"%s","bytes":%s,"mode":"%s","owner_only":%s}' \
    "$separator" "$vault_name_safe" "$vault_state" "$vault_bytes" \
    "$sanitized" "$vault_owner_only"
  separator=,
}

printf '],"vaults":['
separator=""
vaults_emitted=0
vaults_seen=0
for vault_path in "$stado_home"/*.vault*.json; do
  # An unmatched glob stays literal; a dangling symlink fails -e but not -L.
  if [ ! -e "$vault_path" ] && [ ! -L "$vault_path" ]; then
    continue
  fi
  # The active vault is exactly "*.vault.json". Everything else matching the
  # wider glob is history — a snapshot, a pre-migration copy, an
  # acquisitions file — and belongs in the other section.
  case "$vault_path" in
    *.vault.json) ;;
    *) continue ;;
  esac
  vaults_seen=$((vaults_seen + 1))
  if [ "$vaults_emitted" -lt "$vault_limit" ]; then
    vaults_emitted=$((vaults_emitted + 1))
    emit_vault_file "$vault_path"
  fi
done

printf '],"vaults_seen":%d,"vault_sidecars":[' "$vaults_seen"
separator=""
sidecars_emitted=0
sidecars_seen=0
for vault_path in "$stado_home"/*.vault*.json; do
  if [ ! -e "$vault_path" ] && [ ! -L "$vault_path" ]; then
    continue
  fi
  case "$vault_path" in
    *.vault.json) continue ;;
  esac
  sidecars_seen=$((sidecars_seen + 1))
  if [ "$sidecars_emitted" -lt "$vault_limit" ]; then
    sidecars_emitted=$((sidecars_emitted + 1))
    emit_vault_file "$vault_path"
  fi
done

printf '],"vault_sidecars_seen":%d,"sanitizer_state":"%s"}\n' \
  "$sidecars_seen" "$sanitizer_state"
