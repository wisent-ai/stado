//! How old a host's copy of a state is: the newest modification time of the
//! file, or of anything in the tree, read on that host. A take from a source
//! that does not answer starts from the copies the destination holds, and
//! every write the source took after a copy's age is not in it; the take's
//! receipt names each age so whoever takes knows what was lost.

use super::{full_path_lines, root_payload};
use crate::cli::placement::transfer::{marker_line, run_host_script};
use crate::cli::CmdError;
use crate::deploy::Runner;
use crate::placement::PlacementState;
use crate::targets::ComputeTarget;

const NEWEST: &str = "STADO_PLACEMENT_STATE_NEWEST\t";

/// The newest modification time, in Unix seconds, of `state` on `target`:
/// the file's own, or the newest of every file in the tree. `None` when the
/// tree holds no file. A state that is absent is refused naming it.
pub(in crate::cli::placement::transfer) async fn state_newest(
    target: &ComputeTarget,
    state: &PlacementState,
    runner: &Runner,
) -> Result<Option<i64>, CmdError> {
    let locate = full_path_lines(&root_payload(target, state)?, &state.path);
    // BSD and GNU stat name the modification time differently; find prints
    // one line per file and the newest is the largest.
    let script = format!(
        r#"set -eu
{locate}
[ -e "$full" ] || {{ printf '%s is absent\n' "$full" >&2; exit 66; }}
case "$(/usr/bin/uname -s)" in Darwin) format='-f %m' ;; *) format='-c %Y' ;; esac
newest=$(/usr/bin/find "$full" -type f -exec /usr/bin/stat $format {{}} + | /usr/bin/sort -n | /usr/bin/tail -n 1)
printf '{NEWEST}%s\n' "$newest"
"#
    );
    let output = run_host_script(target, &script, runner, "state age").await?;
    let line = marker_line(&output, NEWEST).ok_or_else(|| {
        CmdError::declaration(format!(
            "{} answered no modification time for {}",
            target.name, state.path
        ))
    })?;
    let seconds = line.trim_start_matches(NEWEST).trim();
    if seconds.is_empty() {
        return Ok(None);
    }
    seconds.parse::<i64>().map(Some).map_err(|error| {
        CmdError::declaration(format!(
            "{} answered {seconds:?} as the modification time of {}: {error}",
            target.name, state.path
        ))
    })
}
