/// Phase three, tree shape: replace the code in the install root out of the
/// verified staging tree, one rename per path, and leave every declared
/// host-local path exactly where it is.
///
/// The preserved paths are never named as a destination and never moved: a
/// delivery that relocated `recordings/` and put it back would be one failure
/// away from losing it. They are checked against the staged tree again here,
/// because this is the phase that can destroy state and it must refuse on its
/// own evidence rather than on the caller's ordering. The retired paths are
/// kept beside the staging tree for the same reason the staging tree is kept.
pub const TREE_ACTIVATE_BODY: &str = r##"
stado_release_step=activate
stado_home="$HOME/.stado"
staged_dir="$stado_home/releases/$binary/$version/$platform"
staged_root="$staged_dir/tree"
retired="$staged_dir/retired"

if [ -L "$staged_root" ] || [ ! -d "$staged_root" ]; then
  say verify staged_missing
  exit 1
fi
read_version_file "$staged_root/$version_path"
if [ "$read_version_state" != reported ] || [ "$read_version_value" != "$version" ]; then
  say verify staged_version_mismatch
  exit 1
fi
if [ -L "$install_root" ]; then
  say verify install_root_symlink
  exit 1
fi
if [ -e "$install_root" ] && [ ! -d "$install_root" ]; then
  say verify install_root_not_directory
  exit 1
fi
say verify ok

/bin/mkdir -p "$install_root"
/bin/rm -rf "$retired"
/bin/mkdir -p "$retired"

for staged_entry in "$staged_root"/* "$staged_root"/.*; do
  [ -e "$staged_entry" ] || continue
  entry=${staged_entry##*/}
  case "$entry" in
    . | ..) continue ;;
  esac
  while IFS= read -r preserved; do
    [ -n "$preserved" ] || continue
    if [ "$entry" = "$preserved" ]; then
      say activate "artifact_carries_preserved_path_$entry"
      exit 1
    fi
  done <<EOF
$preserve
EOF
  incoming="$install_root/.$entry.incoming"
  /bin/rm -rf "$incoming"
  /bin/cp -Rp "$staged_entry" "$incoming"
  if [ -e "$install_root/$entry" ]; then
    /bin/mv -f "$install_root/$entry" "$retired/$entry"
  fi
  /bin/mv -f "$incoming" "$install_root/$entry"
  say replaced "$entry"
done

while IFS= read -r preserved; do
  [ -n "$preserved" ] || continue
  say preserved "$preserved"
done <<EOF
$preserve
EOF

# The delivered tree, asked what it is now. A program is proven to run before
# it is activated; a tree is proven to declare the delivered version after it
# is, because the tree in the install root is the one an operator and
# `service converge` will read next.
read_version_file "$install_root/$version_path"
if [ "$read_version_state" != reported ] || [ "$read_version_value" != "$version" ]; then
  say verify installed_version_mismatch
  exit 1
fi
say activated "$version"

# The receipt, beside the artefact it describes.
#
# `converge` can already prove WHETHER these bytes were delivered, by
# comparing the installed file against this staged copy. It could not say
# WHO, and on 2026-08-31 that is exactly where an investigation stopped: a
# `stado` answering 0.13.19 appeared in `$HOME/.stado/bin` on the always-on
# Mac at 21:25Z, the release channel was ruled out, both operator sessions
# were ruled out, the repository's automation was ruled out, and nothing on
# the host recorded who had installed it.
#
# It lives in the version/platform directory rather than beside the active
# binary, so a receipt cannot outlive the artefact it describes or be read
# for a different one. A delivery made before this format simply has none,
# and its absence means "installed before receipts", never "suspicious":
# `staged-match` and `no-staged-copy` answer correctly without it.
receipt_dir="$stado_home/releases/$binary/$version/$platform"
if [ -d "$receipt_dir" ]; then
  printf '{"binary":"%s","version":"%s","platform":"%s","source_commit":"%s","sha256":"%s","installed_at":"%s","delivered_by":"%s"}\n' \
    "$binary" "$version" "$platform" "${source_commit:-}" "${expected_sha256:-}" \
    "$(/bin/date -u +%Y-%m-%dT%H:%M:%SZ)" "${delivered_by:-}" \
    > "$receipt_dir/release-receipt.json" 2>/dev/null || true
  /bin/chmod 600 "$receipt_dir/release-receipt.json" 2>/dev/null || true
fi
say step activate
"##;
