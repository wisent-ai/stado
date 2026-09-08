/// Phase two, tree shape: fetch the release archive, verify its catalog
/// digest, take exactly the declared payload member out of it, unpack that
/// payload into a versioned staging tree, refuse a payload that carries a
/// host-local path, and verify the version the staged tree declares.
///
/// Nothing in this phase touches the install root, which is what makes the
/// ordering structural: a failure here leaves the running tree exactly as it
/// was, because the running tree has not been opened.
pub const TREE_STAGE_BODY: &str = r##"
stado_release_step=stage
stado_home="$HOME/.stado"
staged_dir="$stado_home/releases/$binary/$version/$platform"
staged_root="$staged_dir/tree"
archive_path="$staged_dir/.$archive_name.incoming"
payload_path="$staged_dir/.payload.incoming"
incoming="$staged_dir/.tree.incoming"

# The plan enforced the scheme contract before this script existed: HTTPS
# for every target, loopback HTTP only for a host delivering from its own
# store. This guard is the host-side tripwire for the same shapes.
case "$release_api" in
  https://*|http://127.*|http://localhost|http://localhost:*|http://\[::1\]|http://\[::1\]:*) ;;
  *) say fetch refused_not_https; exit 1 ;;
esac
for required in /usr/bin/curl /usr/bin/openssl /usr/bin/tar /usr/bin/awk /usr/bin/tr /usr/bin/wc; do
  if [ ! -x "$required" ]; then
    say fetch "missing_${required##*/}"
    exit 1
  fi
done

/bin/mkdir -p "$staged_dir"
/bin/rm -f "$archive_path" "$payload_path"
/bin/rm -rf "$incoming"
if ! fetch_release_object \
  "stado://releases/$product/$version/$platform/$archive_name" \
  "$archive_path"; then
  /bin/rm -f "$archive_path"
  exit 1
fi

digest_line=$(/usr/bin/openssl dgst -sha256 -r "$archive_path")
actual_sha256=${digest_line%% *}
say sha256 "$actual_sha256"
if [ "$actual_sha256" != "$expected_sha256" ]; then
  /bin/rm -f "$archive_path"
  say verify mismatch
  exit 1
fi
say verify ok

member_count=0
while IFS= read -r archive_entry; do
  if [ "$archive_entry" = "$member" ]; then
    member_count=$((member_count + 1))
  fi
done <<EOF
$(/usr/bin/tar -tzf "$archive_path")
EOF
if [ "$member_count" -ne 1 ]; then
  /bin/rm -f "$archive_path"
  say layout "archive_member_count_${member_count}_expected_${member}_in_${archive_name}"
  exit 1
fi
if ! /usr/bin/tar -xOzf "$archive_path" "$member" > "$payload_path"; then
  /bin/rm -f "$archive_path" "$payload_path"
  say layout archive_extract_failed
  exit 1
fi
/bin/rm -f "$archive_path"
if [ ! -s "$payload_path" ]; then
  /bin/rm -f "$payload_path"
  say layout empty
  exit 1
fi

/bin/mkdir -p "$incoming"
if ! /usr/bin/tar -xzf "$payload_path" -C "$incoming" --no-same-owner; then
  /bin/rm -rf "$incoming"
  /bin/rm -f "$payload_path"
  say layout payload_extract_failed
  exit 1
fi
/bin/rm -f "$payload_path"

# The payload is code, and only code. A member landing on a declared
# host-local path would be delivered over recordings or scratch state no
# release produced, so such an artefact is refused whole here rather than
# discovered halfway through an activation.
while IFS= read -r preserved; do
  [ -n "$preserved" ] || continue
  if [ -e "$incoming/$preserved" ]; then
    /bin/rm -rf "$incoming"
    say layout "artifact_carries_preserved_path_$preserved"
    exit 1
  fi
done <<EOF
$preserve
EOF

read_version_file "$incoming/$version_path"
if [ "$read_version_state" != reported ]; then
  /bin/rm -rf "$incoming"
  say layout "$read_version_state"
  exit 1
fi
if [ "$read_version_value" != "$version" ]; then
  /bin/rm -rf "$incoming"
  say layout version_mismatch
  exit 1
fi
say layout ok

/bin/rm -rf "$staged_root"
/bin/mv -f "$incoming" "$staged_root"
say staged "$version"
say step stage
"##;
