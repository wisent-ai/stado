use crate::cli::CmdError;

use crate::cli::host::checks::probes::print_json;

/// Remove one file from TARGET's home: the path Stado never had a way to
/// delete, so a retired or broken unit left its plist on disk forever and the
/// only answer was a bare `rm` over ssh, which nothing bounds and nobody
/// audits. This is that answer as a product verb. The guards are on the host,
/// not on the client, because the file is what the host says it is, not what
/// the operator believes:
///
/// - the path must be absolute, contain no `..`, and live under
///   `$HOME/Library/LaunchAgents` or `$HOME/.stado` of the approved account —
///   a system path is not refused because it is dangerous, it is refused
///   because this channel has no right there, and the refusal names the
///   privileged command that does have one;
/// - it must be a regular file owned by that account — a symlink under an
///   allowed root can point anywhere, a directory would make this a recursive
///   delete, and somebody else's file is not this login's to remove.
///
/// Absence is reported as `absent`, not invented into a success.
/// The outcome of one [`remove_file_document`] call, so a composed command
/// (`service remove`) can carry the file half as data instead of scraping
/// another command's stdout.
pub struct RemoveFileOutcome {
    pub target: String,
    pub path: String,
    pub status: String,
    pub detail: Option<String>,
}

impl RemoveFileOutcome {
    pub fn succeeded(&self) -> bool {
        self.status == "removed" || self.status == "absent"
    }

    fn failure_sentence(&self) -> String {
        format!(
            "{}: {} {}{}",
            self.target,
            self.path,
            self.status,
            self.detail
                .as_ref()
                .map(|detail| format!(" — {detail}"))
                .unwrap_or_default()
        )
    }
}

/// The guarded delete itself, as a value: validation, resolution, the fixed
/// remote script, the marker read. Printing belongs to the caller.
pub async fn remove_file_document(target: &str, path: &str) -> Result<RemoveFileOutcome, CmdError> {
    if !path.starts_with('/') || path.contains("..") || path.contains('\0') {
        return Err(CmdError::usage(
            "path must be absolute, contain no '..', and carry no NUL",
        ));
    }
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let quoted = crate::deploy::shlex_quote(path);
    let script = format!(
        r#"set -u
path={quoted}
case "$path" in
  /Library/LaunchDaemons/com.wisent.*.plist|/etc/systemd/system/com.wisent.*.service)
    set -- /usr/bin/sudo -n python3 - "$path" "$HOME" ;;
  *) set -- python3 - "$path" "$HOME" ;;
esac
"$@" <<'STADO_REMOVE_FILE_PROGRAM'
{program}
STADO_REMOVE_FILE_PROGRAM
"#,
        program = include_str!("../../../host_payloads/remove_file/operation.py"),
    );
    let output = crate::deploy::host_channel::run_script_with_timeout(
        &resolved,
        &script,
        std::time::Duration::from_secs(60),
        &crate::deploy::production_runner(),
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    let (state, detail) = output
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("STADO_REMOVE_FILE\t"))
        .and_then(|line| serde_json::from_str::<(String, Option<String>)>(line).ok())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{}: the host answered without a removal report: {}",
                resolved.name,
                crate::deploy::host_channel::last_error_line(&output, "no marker in output")
            ))
        })?;
    let outcome = RemoveFileOutcome {
        target: resolved.name.clone(),
        path: path.to_string(),
        status: state,
        detail,
    };
    if outcome.succeeded() {
        Ok(outcome)
    } else {
        Err(CmdError::click(outcome.failure_sentence()))
    }
}

/// `stado host remove-run-directory TARGET PATH [--json]` — recursively
/// remove exactly one direct child of the managed run root.
///
/// This is deliberately separate from `remove-file`: recursive deletion has a
/// smaller path boundary, and making `rm -rf` an option on the wider
/// single-file command would weaken the guard every existing caller relies on.
pub async fn remove_run_directory(
    target: &str,
    path: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    crate::deploy::host_run::validate_run_directory(path)
        .map_err(|error| CmdError::usage(error).machine_readable(json_output))?;
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()).machine_readable(json_output))?;
    let outcome = crate::deploy::host_run::remove_run_directory(
        &resolved,
        path,
        &crate::deploy::production_runner(),
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()).machine_readable(json_output))?;
    if !outcome.succeeded() {
        return Err(CmdError::click(outcome.failure_sentence()).machine_readable(json_output));
    }
    if json_output {
        print_json(&serde_json::to_value(&outcome)?);
    } else {
        println!("{}: {} {}", outcome.target, outcome.path, outcome.status);
    }
    Ok(())
}
