//! Build and attached execution inside one target account's managed run area.
//!
//! The caller names only a registry target and paths below that target's
//! `$HOME/.stado/work/runs`. Paths are checked once before host resolution for
//! obvious misuse and again on the host against the login account's real home.
//! The host-side check refuses symlinked ancestors, foreign ownership, and a
//! file of the wrong kind before a compiler or program starts.
//!
//! Layout: `validate` holds the lexical refusals applied to caller text,
//! `build` the release build, `attached` the attached execution and its
//! signal forwarding, `remove` the recursive removal of one run directory.
//! This module owns the managed-area constants, the timeouts, and the
//! host-side confinement prelude the build and attached scripts share.

use std::time::Duration;

use super::shlex_quote;

mod attached;
mod build;
mod remove;
mod validate;

pub use attached::{run_attached, AttachedOutcome};
pub use build::{build, BuildOutcome};
pub use remove::{remove_run_directory, RemoveRunDirectoryOutcome};
pub use validate::{
    validate_arguments, validate_binary_name, validate_run_descendant, validate_run_directory,
};

const RUN_AREA: &str = ".stado/work/runs";
const SIGNAL_AREA: &str = ".stado/work/run-signals";
const BUILD_TIMEOUT: Duration = Duration::from_secs(45 * 60);
const SIGNAL_TIMEOUT: Duration = Duration::from_secs(20);
const PATH_REFUSAL: &str =
    "must be an absolute path below the target account's $HOME/.stado/work/runs, with no '.' or '..' component";

/// Host-side confinement shared by build and attached execution. `kind_test`
/// is fixed by this module (`-f` or `-x`), never caller text.
fn confined_file_prelude(path: &str, kind_test: &str, role: &str) -> String {
    let path = shlex_quote(path);
    format!(
        r#"path={path}
declared_home=${{HOME%/}}
case "$path" in
  "$declared_home/{RUN_AREA}/"*) ;;
  *) printf '%s\n' '{role} path is outside the managed run area: expected $HOME/{RUN_AREA}/...' >&2; exit 64 ;;
esac
for component in "$declared_home/.stado" "$declared_home/.stado/work" "$declared_home/{RUN_AREA}"; do
  if [ -L "$component" ]; then printf '%s\n' "managed run ancestor is a symlink: $component" >&2; exit 64; fi
  if [ ! -d "$component" ]; then printf '%s\n' "managed run ancestor is not a directory: $component" >&2; exit 64; fi
  if [ ! -O "$component" ]; then printf '%s\n' "managed run ancestor is not owned by this account: $component" >&2; exit 64; fi
done
if [ -L "$path" ]; then printf '%s\n' '{role} path is a symlink' >&2; exit 64; fi
if [ ! {kind_test} "$path" ]; then printf '%s\n' '{role} path is not an eligible file' >&2; exit 66; fi
if [ ! -O "$path" ]; then printf '%s\n' '{role} path is not owned by this account' >&2; exit 64; fi
parent=${{path%/*}}
physical_home=$(cd -P -- "$declared_home" && /bin/pwd -P) || exit 64
physical_parent=$(cd -P -- "$parent" && /bin/pwd -P) || exit 64
relative_parent=${{parent#"$declared_home"/}}
if [ "$relative_parent" = "$parent" ] || [ "$physical_parent" != "$physical_home/$relative_parent" ]; then
  printf '%s\n' '{role} path crosses a symlinked ancestor outside the managed run area' >&2
  exit 64
fi
"#
    )
}
