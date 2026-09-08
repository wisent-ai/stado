/// Phase one, tree shape: what is in the install root right now.
///
/// Read-only, like its program counterpart, and it reads one thing more: the
/// top-level paths the install root holds, split into the code a delivery
/// would replace and the host-local state it must leave alone. The dry run's
/// promise is about paths on this host, so the paths come off this host
/// rather than out of an assumption on the control plane.
pub const TREE_PROBE_BODY: &str = r##"
stado_release_step=probe
stado_home="$HOME/.stado"
staged_root="$stado_home/releases/$binary/$version/$platform/tree"

# The host's own platform, from the kernel, in the spelling
# bootstrap's remote install script uses. A plan built for one platform must
# never be applied on another, and the host is the only authority on which
# one it is.
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) host_platform=darwin-arm64 ;;
  Linux-x86_64) host_platform=linux-amd64 ;;
  *) host_platform=unsupported ;;
esac
say platform "$host_platform"

if [ -L "$install_root" ]; then
  root_state=refused_symlink
elif [ -d "$install_root" ]; then
  root_state=present
elif [ -e "$install_root" ]; then
  root_state=not_directory
else
  root_state=absent
fi
say root_state "$root_state"

read_version_file "$install_root/$version_path"
say active_state "$read_version_state"
say active_version "$read_version_value"

if [ -L "$staged_root" ]; then
  staged_state=refused_symlink
elif [ -d "$staged_root" ]; then
  staged_state=present
elif [ -e "$staged_root" ]; then
  staged_state=not_directory
else
  staged_state=absent
fi
say staged_state "$staged_state"

if [ "$root_state" = present ]; then
  for entry_path in "$install_root"/* "$install_root"/.*; do
    [ -e "$entry_path" ] || continue
    entry=${entry_path##*/}
    case "$entry" in
      . | ..) continue ;;
    esac
    entry_kind=code
    while IFS= read -r preserved; do
      [ -n "$preserved" ] || continue
      if [ "$entry" = "$preserved" ]; then
        entry_kind=preserved
      fi
    done <<EOF
$preserve
EOF
    if [ "$entry_kind" = preserved ]; then
      say preserved_path "$entry"
    else
      say code_path "$entry"
    fi
  done
fi
say sanitizer "$sanitizer_state"
say step probe
"##;
