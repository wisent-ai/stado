mod activate;
mod probe;
mod stage;

pub use activate::TREE_ACTIVATE_BODY;
pub use probe::TREE_PROBE_BODY;
pub use stage::TREE_STAGE_BODY;

/// The directory name a staged artefact tree is kept under, inside the
/// versioned staging directory the coordinate owns.
pub const TREE_DIR: &str = "tree";

/// The version reader for a tree, added to [`SANITIZE_PRELUDE`] for a tree
/// delivery and to nothing else.
///
/// A tree has no one installed program to ask, so its version comes out of
/// one top-level member of one declared JSON file — `package.json`
/// `/version` for the Weles worker, the same field
/// `weles/.wisent-release.json` numbers the release from. The parsing rules
/// are the ones the program reader already uses on a JSON answer, including
/// the check that only whitespace and the colon sit between the key and its
/// value: `"version"` followed by `null` must read as unparsable, not as
/// whatever the next quoted member happens to be.
pub const TREE_PRELUDE: &str = r##"
read_version_file() {
  read_version_path="$1"
  read_version_value=""
  read_version_state=missing
  # -L first, never -f first: -f follows the link, so a symlink pointing at
  # another product's manifest would be read as this tree's version.
  if [ -L "$read_version_path" ]; then
    read_version_state=refused_symlink
    return 0
  fi
  if [ ! -f "$read_version_path" ]; then
    return 0
  fi
  if read_version_output=$(/bin/cat "$read_version_path" 2>/dev/null); then
    :
  else
    read_version_state=version_failed
    return 0
  fi
  if [ -z "$read_version_output" ]; then
    read_version_state=version_empty
    return 0
  fi
  read_version_key="\"$version_member\""
  case "$read_version_output" in
    *"$read_version_key"*)
      read_version_rest=${read_version_output#*"$read_version_key"}
      read_version_gap=${read_version_rest%%'"'*}
      case "$read_version_rest" in
        *'"'*)
          case "$read_version_gap" in
            *[!:[:space:]]*) ;;
            *)
              read_version_rest=${read_version_rest#*'"'}
              read_version_value=${read_version_rest%%'"'*}
              ;;
          esac
          ;;
      esac
      ;;
  esac
  if [ -z "$read_version_value" ]; then
    read_version_state=version_unparsable
  else
    read_version_state=reported
  fi
}
"##;
