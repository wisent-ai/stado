//! The durable files a profile declares: read from the fenced source and
//! installed on the destination behind a backup the rollback can restore.
//!
//! A state path is relative to its root on each host separately: the home of
//! the account Stado runs as, or the work root the registry declares for that
//! host, so a store kept under a large volume on one host lands under the
//! large volume of the other.

use base64::{engine::general_purpose::STANDARD, Engine as _};

use super::{marker_line, run_host_script, StateSnapshot};
use crate::cli::CmdError;
use crate::deploy::Runner;
use crate::placement::{PlacementState, StateRoot};
use crate::targets::ComputeTarget;

fn payload(text: &str) -> String {
    STANDARD.encode(text.as_bytes())
}

/// How the state is written in messages: `$HOME/<path>` or
/// `<work root>/<path>`.
pub(super) fn state_location(state: &PlacementState) -> String {
    match state.root {
        StateRoot::Home => format!("$HOME/{}", state.path),
        StateRoot::Work => format!("<work root>/{}", state.path),
    }
}

/// The directory the state's path is under on `target`, as a payload the
/// host script decodes: empty for the home, which the script reads from the
/// account's own `$HOME`; the declared work root otherwise. A host that
/// declares no work root cannot hold a work-rooted state, and is refused
/// before anything on it is touched.
pub(super) fn root_payload(
    target: &ComputeTarget,
    state: &PlacementState,
) -> Result<String, CmdError> {
    match state.root {
        StateRoot::Home => Ok(String::new()),
        StateRoot::Work => target.work_root.as_deref().map(payload).ok_or_else(|| {
            CmdError::click(format!(
                "{}: state {} is kept under the work root, and the registry declares none for \
                 {}; declare targets.{}.work_root before moving it there",
                target.name, state.path, target.name, target.name
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        }),
    }
}

/// The script lines that set `full` to the state's file on the host.
fn full_path_lines(root: &str, path: &str) -> String {
    format!(
        r#"case "$(/usr/bin/uname -s)" in Darwin) decode=-D ;; *) decode=--decode ;; esac
relative=$(printf '%s' '{path}' | /usr/bin/base64 "$decode")
root=$(printf '%s' '{root}' | /usr/bin/base64 "$decode")
[ -n "$root" ] || root="$HOME"
full="$root/$relative""#,
        path = payload(path)
    )
}

pub(super) async fn state_exists(
    target: &ComputeTarget,
    state: &PlacementState,
    runner: &Runner,
) -> Result<bool, CmdError> {
    let locate = full_path_lines(&root_payload(target, state)?, &state.path);
    let script = format!(
        r#"set -eu
{locate}
if [ -f "$full" ]; then printf 'STADO_PLACEMENT_STATE\tpresent\n';
elif [ -e "$full" ]; then printf '%s is not a regular file\n' "$full" >&2; exit 65;
else printf 'STADO_PLACEMENT_STATE\tmissing\n'; fi
"#
    );
    let output = run_host_script(target, &script, runner, "state preflight").await?;
    Ok(marker_line(&output, "STADO_PLACEMENT_STATE\tpresent").is_some())
}

pub(super) async fn read_state(
    target: &ComputeTarget,
    state: &PlacementState,
    runner: &Runner,
) -> Result<StateSnapshot, CmdError> {
    let locate = full_path_lines(&root_payload(target, state)?, &state.path);
    let script = format!(
        r#"set -eu
{locate}
if [ ! -e "$full" ]; then printf 'STADO_PLACEMENT_STATE\tmissing\n'; exit 0; fi
[ -f "$full" ] || {{ printf '%s is not a regular file\n' "$full" >&2; exit 65; }}
payload=$(/usr/bin/base64 < "$full" | /usr/bin/tr -d '\r\n')
printf 'STADO_PLACEMENT_STATE\tpresent\t%s\n' "$payload"
"#
    );
    let output = run_host_script(target, &script, runner, "state read").await?;
    let line = marker_line(&output, "STADO_PLACEMENT_STATE\t").ok_or_else(|| {
        CmdError::click(format!(
            "{}: state read returned no marker for {}",
            target.name, state.path
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    let mut fields = line.splitn(3, '\t');
    let _marker = fields.next();
    match fields.next() {
        Some("missing") if !state.required => Ok(StateSnapshot {
            spec: state.clone(),
            bytes: None,
        }),
        Some("missing") => Err(CmdError::click(format!(
            "{}: required state {} disappeared after fencing",
            target.name, state.path
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound)),
        Some("present") => {
            let payload = fields.next().unwrap_or_default();
            let bytes = STANDARD.decode(payload).map_err(|error| {
                CmdError::click(format!(
                    "{}: invalid state payload for {}: {error}",
                    target.name, state.path
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown)
            })?;
            Ok(StateSnapshot {
                spec: state.clone(),
                bytes: Some(bytes),
            })
        }
        _ => Err(CmdError::click(format!(
            "{}: malformed state marker for {}",
            target.name, state.path
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)),
    }
}

pub(super) async fn write_state(
    target: &ComputeTarget,
    snapshot: &StateSnapshot,
    transaction_id: &str,
    runner: &Runner,
) -> Result<(), CmdError> {
    let locate = full_path_lines(&root_payload(target, &snapshot.spec)?, &snapshot.spec.path);
    let transaction = STANDARD.encode(transaction_id.as_bytes());
    let (present, payload) = match &snapshot.bytes {
        Some(bytes) => ("yes", STANDARD.encode(bytes)),
        None => ("no", String::new()),
    };
    let script = format!(
        r#"set -eu
umask 077
{locate}
txn=$(printf '%s' '{transaction}' | /usr/bin/base64 "$decode")
backup="$full.pre-stado-placement-$txn"
meta="$backup.meta"
parent=$(/usr/bin/dirname "$full")
/bin/mkdir -p "$parent"
[ ! -e "$backup" ] && [ ! -e "$meta" ] || {{ printf 'placement backup already exists: %s\n' "$backup" >&2; exit 73; }}
had=no
if [ -f "$full" ]; then /bin/cp -p "$full" "$backup"; had=yes;
elif [ -e "$full" ]; then printf '%s is not a regular file\n' "$full" >&2; exit 65; fi
printf '%s\n' "$had" > "$meta"
if [ '{present}' = yes ]; then
  tmp="$full.placement-$txn.tmp"
  trap '/bin/rm -f "$tmp"' EXIT HUP INT TERM
  printf '%s' '{payload}' | /usr/bin/base64 "$decode" > "$tmp"
  /bin/chmod 600 "$tmp"
  /bin/mv -f "$tmp" "$full"
else
  /bin/rm -f "$full"
fi
printf 'STADO_PLACEMENT_WRITE\tok\t%s\n' "$had"
"#
    );
    let output = run_host_script(target, &script, runner, "state install").await?;
    if marker_line(&output, "STADO_PLACEMENT_WRITE\tok\t").is_none() {
        return Err(CmdError::click(format!(
            "{}: state install returned no marker for {}",
            target.name, snapshot.spec.path
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    Ok(())
}

pub(super) async fn restore_state(
    target: &ComputeTarget,
    state: &PlacementState,
    transaction_id: &str,
    runner: &Runner,
) -> Result<(), CmdError> {
    let path = &state.path;
    let locate = full_path_lines(&root_payload(target, state)?, path);
    let transaction = STANDARD.encode(transaction_id.as_bytes());
    let script = format!(
        r#"set -eu
{locate}
txn=$(printf '%s' '{transaction}' | /usr/bin/base64 "$decode")
backup="$full.pre-stado-placement-$txn"
meta="$backup.meta"
if [ ! -f "$meta" ]; then
  printf 'STADO_PLACEMENT_RESTORE\tok\tuntouched\n'
  exit 0
fi
had=$(/bin/cat "$meta")
if [ "$had" = yes ]; then
  [ -f "$backup" ] || {{ printf 'placement backup is missing: %s\n' "$backup" >&2; exit 74; }}
  /bin/mv -f "$backup" "$full"
else
  /bin/rm -f "$full"
fi
/bin/rm -f "$meta"
printf 'STADO_PLACEMENT_RESTORE\tok\n'
"#
    );
    let output = run_host_script(target, &script, runner, "state rollback").await?;
    if marker_line(&output, "STADO_PLACEMENT_RESTORE\tok").is_none() {
        return Err(CmdError::click(format!(
            "{}: state rollback returned no marker for {path}",
            target.name
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    Ok(())
}

pub(in crate::cli::placement) async fn cleanup_state_backup(
    target: &ComputeTarget,
    state: &PlacementState,
    transaction_id: &str,
    runner: &Runner,
) -> Result<(), CmdError> {
    let path = &state.path;
    let locate = full_path_lines(&root_payload(target, state)?, path);
    let transaction = STANDARD.encode(transaction_id.as_bytes());
    let script = format!(
        r#"set -eu
{locate}
txn=$(printf '%s' '{transaction}' | /usr/bin/base64 "$decode")
/bin/rm -f "$full.pre-stado-placement-$txn" "$full.pre-stado-placement-$txn.meta"
printf 'STADO_PLACEMENT_CLEANUP\tok\n'
"#
    );
    let output = run_host_script(target, &script, runner, "backup cleanup").await?;
    if marker_line(&output, "STADO_PLACEMENT_CLEANUP\tok").is_none() {
        return Err(CmdError::click(format!(
            "{}: backup cleanup returned no marker for {path}",
            target.name
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    Ok(())
}
