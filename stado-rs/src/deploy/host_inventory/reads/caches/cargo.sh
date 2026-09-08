# Cargo home metadata and complete bin membership belong to the typed host
# inventory, not to `host exec`: the paths are fixed at the managed account's
# own `$HOME`, names are sanitized, and each reported entry is lstat'd. A
# symlink at `$HOME/.cargo` is itself reported with its link target, then its
# fixed `bin` child is traversed: Cargo installations commonly place that
# whole tree on a mounted cache, and this read-only inventory opens no child
# contents.
printf '],"cargo":{"home":'
emit_filesystem_metadata "$cargo_home" '.cargo'
cargo_home_complete=$metadata_complete
cargo_home_kind=$metadata_kind
printf ',"bin":'
case "$cargo_home_kind" in
  directory|symlink)
    emit_filesystem_metadata "$cargo_bin" 'bin'
    ;;
  missing)
    # If the fixed parent does not exist, its fixed child does not exist
    # either; no second filesystem traversal is needed to state that.
    emit_filesystem_metadata "$cargo_bin" 'bin'
    ;;
  *)
    emit_refused_filesystem_metadata 'bin' refused_parent_not_directory
    ;;
esac
cargo_bin_complete=$metadata_complete
cargo_bin_kind=$metadata_kind
printf ',"entries":['
separator=""
cargo_entries_seen=0
cargo_entries_emitted=0
cargo_entries_complete=true
cargo_entries_state=missing
if [ "$cargo_home_kind" != directory ] && [ "$cargo_home_kind" != symlink ] && \
   [ "$cargo_home_kind" != missing ]; then
  cargo_entries_state=refused_parent_not_directory
  cargo_entries_complete=false
elif [ "$cargo_bin_kind" = directory ] || [ "$cargo_bin_kind" = symlink ]; then
  if [ ! -d "$cargo_bin" ]; then
    cargo_entries_state=refused_not_directory
    cargo_entries_complete=false
  elif [ ! -r "$cargo_bin" ]; then
    cargo_entries_state=refused_unreadable
    cargo_entries_complete=false
  elif ! /usr/bin/find -H "$cargo_bin" -mindepth 1 -maxdepth 1 -print >/dev/null 2>&1; then
    # Globs do not expose a traversal status. A fixed, depth-one walk does,
    # so an I/O or permission failure cannot become a complete empty list.
    cargo_entries_state=partial_traversal
    cargo_entries_complete=false
  else
    cargo_entries_state=read
    # `*` excludes dotfiles. The two disjoint dot patterns add every real
    # hidden member without ever including the synthetic `.` and `..`.
    for cargo_entry_path in "$cargo_bin"/* "$cargo_bin"/.[!.]* "$cargo_bin"/..?*; do
      # An unmatched glob stays literal; a dangling symlink fails -e but not -L.
      if [ ! -e "$cargo_entry_path" ] && [ ! -L "$cargo_entry_path" ]; then
        continue
      fi
      cargo_entries_seen=$((cargo_entries_seen + 1))
      if [ "$cargo_entries_emitted" -lt "$cargo_entry_limit" ]; then
        cargo_entries_emitted=$((cargo_entries_emitted + 1))
        cargo_entry_name=${cargo_entry_path##*/}
        printf '%s' "$separator"
        emit_filesystem_metadata "$cargo_entry_path" "$cargo_entry_name"
        if [ "$metadata_complete" != true ] || [ "$metadata_state" != read ]; then
          cargo_entries_complete=false
        fi
        separator=,
      else
        cargo_entries_complete=false
      fi
    done
  fi
elif [ "$cargo_bin_kind" != missing ]; then
  cargo_entries_state=refused_not_directory
  cargo_entries_complete=false
fi
cargo_complete=true
if [ "$cargo_home_complete" != true ] || [ "$cargo_bin_complete" != true ] || \
   [ "$cargo_entries_complete" != true ]; then
  cargo_complete=false
fi
printf '],"entries_seen":%d,"entries_complete":%s,"entries_state":"%s","complete":%s}' \
  "$cargo_entries_seen" "$cargo_entries_complete" "$cargo_entries_state" \
  "$cargo_complete"

