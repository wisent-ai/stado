//! Recursive removal of one complete run directory, re-checked on the host
//! against the login account's real home before anything is unlinked.

use std::time::Duration;

use serde::Serialize;

use crate::deploy::{host_channel, shlex_quote, DeployError, Runner};
use crate::targets::ComputeTarget;

use super::RUN_AREA;

#[derive(Debug, Serialize)]
pub struct RemoveRunDirectoryOutcome {
    pub target: String,
    pub path: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl RemoveRunDirectoryOutcome {
    pub fn succeeded(&self) -> bool {
        self.status == "removed" || self.status == "absent"
    }

    pub fn failure_sentence(&self) -> String {
        format!(
            "{}: {} {}{}",
            self.target,
            self.path,
            self.status,
            self.detail
                .as_ref()
                .filter(|detail| !detail.is_empty())
                .map(|detail| format!(" — {detail}"))
                .unwrap_or_default()
        )
    }
}

pub async fn remove_run_directory(
    target: &ComputeTarget,
    path: &str,
    runner: &Runner,
) -> Result<RemoveRunDirectoryOutcome, DeployError> {
    let quoted = shlex_quote(path);
    let script = format!(
        r#"set -u
path={quoted}
report() {{ printf 'STADO_REMOVE_RUN_DIRECTORY\t%s\t%s\n' "$1" "$2"; }}
declared_home=${{HOME%/}}
case "$path" in
  "$declared_home/{RUN_AREA}/"*) ;;
  *) report refused 'outside the managed run area; expected $HOME/{RUN_AREA}/RUN'; exit 0 ;;
esac
relative=${{path#"$declared_home/{RUN_AREA}/"}}
case "$relative" in ''|*/*) report refused 'a recursive removal must name one direct child of the managed run area'; exit 0 ;; esac
if [ ! -e "$path" ] && [ ! -L "$path" ]; then report absent ''; exit 0; fi
for component in "$declared_home/.stado" "$declared_home/.stado/work" "$declared_home/{RUN_AREA}"; do
  if [ -L "$component" ]; then report refused "managed run ancestor is a symlink: $component"; exit 0; fi
  if [ ! -d "$component" ]; then report refused "managed run ancestor is not a directory: $component"; exit 0; fi
  if [ ! -O "$component" ]; then report refused "managed run ancestor is not owned by this account: $component"; exit 0; fi
done
if [ -L "$path" ]; then report refused 'run directory is a symlink'; exit 0; fi
if [ ! -d "$path" ]; then report refused 'run path is not a directory'; exit 0; fi
if [ ! -O "$path" ]; then report refused 'run directory is not owned by this account'; exit 0; fi
physical_home=$(cd -P -- "$declared_home" && /bin/pwd -P) || {{ report refused 'target home could not be resolved'; exit 0; }}
physical_parent=$(cd -P -- "${{path%/*}}" && /bin/pwd -P) || {{ report refused 'run parent could not be resolved'; exit 0; }}
if [ "$physical_parent" != "$physical_home/{RUN_AREA}" ]; then report refused 'run directory crosses a symlinked ancestor outside the managed run area'; exit 0; fi
/bin/rm -rf -- "$path"
if [ -e "$path" ] || [ -L "$path" ]; then report failed 'rm returned and the run directory is still present'; else report removed ''; fi
"#
    );
    let output =
        host_channel::run_script_with_timeout(target, &script, Duration::from_secs(5 * 60), runner)
            .await?;
    let (status, detail) = output
        .stdout
        .lines()
        .find_map(|line| {
            let fields = host_channel::marker_fields(line);
            (fields.first() == Some(&"STADO_REMOVE_RUN_DIRECTORY") && fields.len() >= 3).then(
                || {
                    (
                        fields[1].to_string(),
                        (!fields[2].is_empty()).then(|| fields[2].to_string()),
                    )
                },
            )
        })
        .ok_or_else(|| {
            DeployError(format!(
                "{}: the host answered without a run-directory removal report: {}",
                target.name,
                host_channel::last_error_line(&output, "no marker in output")
            ))
        })?;
    Ok(RemoveRunDirectoryOutcome {
        target: target.name.clone(),
        path: path.to_string(),
        status,
        detail,
    })
}
