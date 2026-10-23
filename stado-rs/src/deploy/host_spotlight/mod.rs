//! Keep Spotlight out of a host's build caches.
//!
//! Spotlight indexes every file a Mac writes. A build cache rewrites
//! gigabytes of object files with every build, so indexing it costs the host
//! its disk and its CPU while nothing in it is ever searched for. Which
//! directories are build caches is not guessed from their names: it is the
//! same Cache Directory Tagging Standard rule `host_build_caches` judges
//! (a `CACHEDIR.TAG` carrying the standard signature, which Cargo and other
//! build tools write), read from that walk's own verdicts. Spotlight leaves a
//! folder that holds an empty `.metadata_never_index` file unindexed, so each
//! tagged cache without one is reported, and with `apply` the file is
//! written. Only a Mac indexes with Spotlight; on any other platform the walk
//! reports the platform and writes nothing.

use crate::deploy::host_build_caches::{
    home_scan_for_target, report_declaration_on_host, TAGGED_VERDICTS,
};
use crate::deploy::host_channel::target_is_this_host;
use crate::deploy::{host_channel, shlex_quote, CommandSpec, DeployError, Runner};
use crate::targets::ComputeTarget;

/// The file whose presence keeps Spotlight out of the folder holding it.
pub const MARKER: &str = ".metadata_never_index";
/// Marker prefix of the remote script's report lines.
pub const STATUS_PREFIX: &str = "STADO_SPOTLIGHT\t";
const DIRECTORIES_ENV: &str = "STADO_SPOTLIGHT_DIRECTORIES";
const APPLY_ENV: &str = "STADO_SPOTLIGHT_APPLY";

/// One directory and what became of its marker; `cause` is the shell's own
/// sentence for a marker it could not write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotlightEntry {
    pub state: String,
    pub path: String,
    pub cause: Option<String>,
}

/// One host's outcome: every tagged cache's marker state, and why a read
/// failed when one did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotlightReport {
    pub target: String,
    pub entries: Vec<SpotlightEntry>,
    pub error: Option<String>,
}

/// Each directory in `STADO_SPOTLIGHT_DIRECTORIES`: `excluded` when its
/// marker is there, `unmarked` when it is not and nothing is applied,
/// `marked` once written, `mark-failed` with the shell's answer when the
/// write failed, and `not-darwin` once for a host that is not a Mac.
const REMOTE_SCRIPT: &str = r#"apply="${STADO_SPOTLIGHT_APPLY:-}"
if [ "$(/usr/bin/uname 2>/dev/null || /bin/uname)" != "Darwin" ]; then
  printf 'STADO_SPOTLIGHT\tnot-darwin\t%s\n' -
  exit 0
fi
printf '%s\n' "${STADO_SPOTLIGHT_DIRECTORIES:-}" | while IFS= read -r dir; do
  [ -n "$dir" ] || continue
  if [ -e "$dir/.metadata_never_index" ]; then
    printf 'STADO_SPOTLIGHT\texcluded\t%s\n' "$dir"
  elif [ -z "$apply" ]; then
    printf 'STADO_SPOTLIGHT\tunmarked\t%s\n' "$dir"
  elif written=$( { : > "$dir/.metadata_never_index"; } 2>&1 ); then
    printf 'STADO_SPOTLIGHT\tmarked\t%s\n' "$dir"
  else
    printf 'STADO_SPOTLIGHT\tmark-failed\t%s\t%s\n' "$dir" "$written"
  fi
done
"#;

/// The remote invocation over `directories`, writing markers when `apply`.
/// Markers belong to the account whose caches they mark, so nothing is
/// escalated.
pub fn remote_command(directories: &[String], apply: bool) -> String {
    format!(
        "/usr/bin/env {}={} {}={} /bin/sh -c {}",
        DIRECTORIES_ENV,
        shlex_quote(&directories.join("\n")),
        APPLY_ENV,
        shlex_quote(if apply { "apply" } else { "" }),
        shlex_quote(REMOTE_SCRIPT)
    )
}

/// The entries of the script's marked lines; a line without a state and a
/// path is kept as `unreadable` with the line, never dropped, and whatever
/// follows the path is the cause the script gave, whole.
pub fn parse(stdout: &str) -> Vec<SpotlightEntry> {
    stdout
        .lines()
        .filter_map(|line| line.strip_prefix(STATUS_PREFIX))
        .map(|rest| {
            let unreadable = || SpotlightEntry {
                state: "unreadable".to_owned(),
                path: rest.to_owned(),
                cause: None,
            };
            let Some((state, after)) = rest.split_once('\t') else {
                return unreadable();
            };
            let (path, cause) = match after.split_once('\t') {
                Some((path, cause)) => (path, Some(cause.to_owned())),
                None => (after, None),
            };
            match !state.is_empty() && !path.is_empty() {
                true => SpotlightEntry {
                    state: state.to_owned(),
                    path: path.to_owned(),
                    cause,
                },
                false => unreadable(),
            }
        })
        .collect()
}

/// Report, or with `apply` write, the Spotlight marker of every tagged build
/// cache in `target`'s home.
pub async fn exclude_on_host(
    target: &ComputeTarget,
    apply: bool,
    runner: &Runner,
) -> SpotlightReport {
    let mut report = SpotlightReport {
        target: target.name.clone(),
        entries: Vec::new(),
        error: None,
    };
    let declaration = match home_scan_for_target(target, runner).await {
        Ok(declaration) => declaration,
        Err(error) => {
            report.error = Some(format!(
                "the home of {} cannot be read: {}",
                target.name, error.message
            ));
            return report;
        }
    };
    let caches = report_declaration_on_host(target, &declaration, runner).await;
    if let Some(error) = caches.error {
        report.error = Some(format!(
            "the build cache walk of {} did not finish: {error}",
            target.name
        ));
        return report;
    }
    let directories: Vec<String> = caches
        .entries
        .into_iter()
        .filter(|entry| TAGGED_VERDICTS.contains(&entry.state.as_str()))
        .map(|entry| entry.path)
        .collect();
    if directories.is_empty() {
        return report;
    }
    let command = remote_command(&directories, apply);
    let result = if target_is_this_host(target) {
        let shell = vec!["/bin/sh".to_string(), "-c".to_string(), command];
        runner(CommandSpec::new(shell)).await
    } else if !target.has_ssh_connection() {
        report.error = Some(format!(
            "target {} has no SSH connection path and is not this host",
            target.name
        ));
        return report;
    } else {
        host_channel::run_script_to_completion(target, &command, runner)
            .await
            .map_err(|error: DeployError| error.message)
    };
    match result {
        Ok(output) => {
            report.entries = parse(&output.stdout);
            if !output.ok() {
                report.error = Some(output.detail().trim().to_string());
            }
        }
        Err(error) => report.error = Some(error),
    }
    report
}
