//! What a committed move leaves of each state's backup on the destination.

use base64::{engine::general_purpose::STANDARD, Engine as _};

use super::super::{marker_line, run_host_script};
use super::{full_path_lines, root_payload};
use crate::cli::CmdError;
use crate::deploy::Runner;
use crate::placement::PlacementState;
use crate::targets::ComputeTarget;

/// Settle one state's backup after the move committed. A destination that
/// held its own file or tree at the state's path before the move keeps it,
/// renamed to `<path>.replaced-by-placement-<transaction>` beside the moved
/// state, and the kept path is returned so the receipt names it: the move
/// carried the source's state over that path, and what the destination held
/// there was never part of the source, so deleting it would destroy data
/// nobody moved. A path the destination did not hold leaves nothing behind.
pub(in crate::cli::placement) async fn settle_state_backup(
    target: &ComputeTarget,
    state: &PlacementState,
    transaction_id: &str,
    runner: &Runner,
) -> Result<Option<String>, CmdError> {
    let path = &state.path;
    let locate = full_path_lines(&root_payload(target, state)?, path);
    let transaction = STANDARD.encode(transaction_id.as_bytes());
    let script = format!(
        r#"set -eu
{locate}
txn=$(printf '%s' '{transaction}' | /usr/bin/base64 "$decode")
backup="$full.pre-stado-placement-$txn"
meta="$backup.meta"
kept="$full.replaced-by-placement-$txn"
had=no
[ ! -f "$meta" ] || had=$(/bin/cat "$meta")
if [ "$had" = yes ]; then
  [ -e "$backup" ] || {{ printf 'placement backup is missing: %s\n' "$backup" >&2; exit 66; }}
  [ ! -e "$kept" ] || {{ printf 'a kept replaced state already exists: %s\n' "$kept" >&2; exit 73; }}
  /bin/mv "$backup" "$kept"
  /bin/rm -f "$meta"
  printf 'STADO_PLACEMENT_SETTLE\tkept\t%s\n' "$kept"
else
  /bin/rm -rf "$backup" "$meta"
  printf 'STADO_PLACEMENT_SETTLE\tnone\n'
fi
"#
    );
    let output = run_host_script(target, &script, runner, "state settle").await?;
    match marker_line(&output, "STADO_PLACEMENT_SETTLE\t") {
        Some(line) => Ok(line
            .strip_prefix("STADO_PLACEMENT_SETTLE\tkept\t")
            .map(str::to_string)),
        None => Err(CmdError::click(format!(
            "{}: state settle returned no marker for {path}",
            target.name
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)),
    }
}
