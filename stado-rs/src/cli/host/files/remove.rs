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
report() {{ printf 'STADO_REMOVE_FILE\t%s\t%s\n' "$1" "$2"; }}
# A service file is removable exactly where Stado installs service files.
# Keeping this list beside the delete is deliberate: a missing Linux path left
# retired user units on disk, ready for an older coordinator or a manual
# `systemctl enable` to resurrect. Root-owned machine units keep the same
# `com.wisent.*` namespace restriction as LaunchDaemons.
privileged=no
case "$path" in
  "$HOME/Library/LaunchAgents/"*|"$HOME/.stado/"*|"$HOME/.config/systemd/user/"*) ;;
  /Library/LaunchDaemons/com.wisent.*.plist|/etc/systemd/system/com.wisent.*.service) privileged=yes ;;
  *) report refused "outside the managed areas; remove it on the host with: sudo rm -- $path"; exit 0 ;;
esac
if [ -L "$path" ]; then
  report refused "a symlink points outside the managed area; remove it by hand: rm -- $path"
elif [ -d "$path" ]; then
  report refused "a directory is not removed by a single-file command"
elif [ ! -e "$path" ]; then
  report absent ""
elif [ ! -f "$path" ]; then
  report refused "not a regular file"
elif [ "$privileged" = yes ]; then
  # Owned by root by construction, so the `-O` test the home areas use would
  # refuse every one of them. The grant is the same `sudo -n` the install used;
  # a host without it is told which command was refused rather than left with a
  # unit nobody can remove.
  if /usr/bin/sudo -n /bin/rm -f -- "$path"; then
    if [ -e "$path" ]; then
      report failed "sudo rm succeeded and the path is still there"
    else
      report removed ""
    fi
  else
    report refused "sudo -n rm -- $path was refused; this host has no passwordless grant"
  fi
elif [ ! -O "$path" ]; then
  report refused "not owned by this account; remove it on the host with: sudo rm -- $path"
else
  rm -f -- "$path"
  if [ -e "$path" ]; then
    report failed "rm succeeded and the path is still there"
  else
    report removed ""
  fi
fi
"#
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
        .find_map(|line| {
            crate::deploy::host_channel::marker_fields(line)
                .as_slice()
                .split_first()
                .and_then(|(marker, rest)| {
                    (*marker == "STADO_REMOVE_FILE")
                        .then(|| (rest[0].to_string(), rest.get(1).map(|s| s.to_string())))
                })
        })
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
