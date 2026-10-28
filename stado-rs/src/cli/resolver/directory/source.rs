use std::sync::Arc;

use serde_json::Value;

use crate::cli::CmdError;
use crate::service_resolution;
use crate::targets::{self, RegistryStore};

use crate::cli::resolver::authority::execute::{execute, OpenWaits};
use crate::cli::resolver::authority::paths::target_ssh_paths;
use crate::cli::resolver::authority::refusal::refuse_authority;
use crate::cli::resolver::directory::read_local_snapshot;
use crate::cli::resolver::directory::validate_snapshot;
use crate::cli::resolver::directory::SnapshotPayload;

#[derive(Clone)]
pub(crate) enum SnapshotSource {
    Local(Arc<RegistryStore>),
    Authority {
        /// Registry name of the authority target, carried so a failed read
        /// can name the host that is actually silent.
        target: String,
        ssh: Vec<targets::SshConnectionPath>,
        command: String,
    },
}

impl SnapshotSource {
    /// The host a failure of this source is evidence about.
    pub(crate) fn subject_host(&self, local_target: &str) -> String {
        match self {
            Self::Local(_) => local_target.to_string(),
            Self::Authority { target, .. } => target.clone(),
        }
    }

    /// `reader` is the refusal vocabulary's word for who is reading:
    /// [`host_silence::READER_RESOLVER`] for the serving loop and its
    /// background refresh, [`host_silence::READER_CLI`] for a one-shot
    /// command.
    pub(crate) async fn fetch(
        &self,
        reader: &'static str,
    ) -> Result<(Value, String, u64), CmdError> {
        match self {
            Self::Local(store) => read_local_snapshot(store).await,
            // Each authority read owns a native connection independent of adapter traffic.
            // An authority whose services this host's resolver already holds
            // unanswered past its refresh interval is not asked: its snapshot
            // command reads the same store and vault and would stand with
            // them, and every registry reader of this program would stand
            // behind it. The refusal names the host and the held connection,
            // and the reader goes on to its store or its last-known-good copy.
            // A read already sent when the host's services stand is abandoned
            // once the resolver publishes them held, with the same refusal:
            // `stado resolver status` stood on this read for hours.
            Self::Authority {
                target,
                ssh,
                command,
            } => {
                // A read of this authority that another command already gave
                // up on less than an interval ago is not sent again: the
                // reader goes on to its copy at once.
                if let Some(standing) = super::remembered::unanswered::standing(target) {
                    return Err(CmdError::unreachable(standing));
                }
                match crate::cli::resolver::held_on_host(target) {
                    Ok(None) => {}
                    Ok(Some(held)) => return Err(CmdError::unreachable(held)),
                    Err(unreadable) => {
                        return Err(CmdError::unreachable(format!(
                            "whether the registry authority {target} answers cannot be told \
                             before asking it: {unreadable}"
                        )))
                    }
                }
                // Said before it is sent: a command ended at the interval
                // never reaches the abandonment below, and the next reader
                // finds this read standing instead of sending its own.
                let route = crate::cli::resolver::Route::Host(target);
                match crate::cli::resolver::bound_seconds(&route) {
                    Ok(Some(bound)) => super::remembered::unanswered::asking(target, bound.seconds),
                    Ok(None) => {}
                    Err(unreadable) => eprintln!(
                        "stado: the read of the registry authority {target} is sent without \
                         being kept as in flight: {unreadable}"
                    ),
                }
                // One read: a failure is the read's answer, said whole, and
                // the reader goes on to its copy with it. The authority's own
                // wait lines are kept as they arrive, so a read given up on
                // says what the authority itself stood on.
                let open = OpenWaits::default();
                let reads = Self::fetch_authority_paths(target, ssh, command, reader, &open);
                let route_held = crate::cli::resolver::Route::Host(target);
                let (sentence, silent) =
                    match crate::cli::resolver::unless_held(route_held, reads).await {
                        Ok(Ok(snapshot)) => {
                            super::remembered::unanswered::forget(target);
                            return Ok(snapshot);
                        }
                        // The authority answered, with a refusal: it is
                        // reachable, and the next reader asks it again.
                        Ok(Err(unread)) if !unread.silent => {
                            super::remembered::unanswered::answered(target);
                            return Err(CmdError::unreachable(unread.sentence));
                        }
                        // No SSH path reached the host at all: each next
                        // reader would stand the same connection failure, so
                        // it is kept like a read given up on.
                        Ok(Err(unread)) => (unread.sentence, true),
                        Err(held) => (format!("{held}; {}", open.said()), false),
                    };
                // The interval measures how long the record stands; an
                // interval that cannot be read is added to the refusal
                // instead of the read being remembered.
                match crate::cli::resolver::bound_seconds(&route) {
                    Ok(Some(bound)) => {
                        super::remembered::unanswered::remember(target, bound.seconds, &sentence)
                    }
                    Ok(None) if silent => super::remembered::unanswered::answered(target),
                    Ok(None) => {}
                    Err(unreadable) => {
                        return Err(CmdError::unreachable(format!(
                            "{sentence}; it is not remembered as unanswered: {unreadable}"
                        )))
                    }
                }
                Err(CmdError::unreachable(sentence))
            }
        }
    }

    /// Ask every SSH path of the authority at once and take the first
    /// snapshot that answers. The paths were asked one after another, so a
    /// declared first path that accepts connections and answers nothing held
    /// every registry read for as long as it stood before a path that answers
    /// was tried at all. The reads still standing when one answers are
    /// dropped, and each ends its wait saying it was cut short. When none
    /// answers, every path's own failure is named, in the declared order, and
    /// the read is `silent` when no path reached the host at all.
    async fn fetch_authority_paths(
        target: &str,
        paths: &[targets::SshConnectionPath],
        command: &str,
        reader: &'static str,
        open: &OpenWaits,
    ) -> Result<(Value, String, u64), AuthorityUnread> {
        use futures::stream::{FuturesUnordered, StreamExt};

        if paths.is_empty() {
            return Err(AuthorityUnread {
                sentence: "registry authority has no SSH connection path".to_string(),
                silent: false,
            });
        }
        let mut reads = paths
            .iter()
            .enumerate()
            .map(|(order, path)| async move {
                let read =
                    Self::fetch_authority(target, &path.destination, command, reader, open).await;
                (order, path, read)
            })
            .collect::<FuturesUnordered<_>>();
        let mut failures = Vec::with_capacity(paths.len());
        let mut silent = true;
        while let Some((order, path, read)) = reads.next().await {
            match read {
                Ok(snapshot) => return Ok(snapshot),
                Err(failure) => {
                    silent &= failure.silent;
                    failures.push((order, format!("{}: {}", path.name, failure.sentence)));
                }
            }
        }
        failures.sort_by_key(|(order, _)| *order);
        let named: Vec<String> = failures.into_iter().map(|(_, failure)| failure).collect();
        Err(AuthorityUnread {
            sentence: format!(
                "no registry authority SSH connection path answered ({})",
                named.join("; ")
            ),
            silent,
        })
    }

    async fn fetch_authority(
        target: &str,
        ssh: &str,
        command: &str,
        reader: &'static str,
        open: &OpenWaits,
    ) -> Result<(Value, String, u64), AuthorityUnread> {
        let remote_command = format!("{} resolver snapshot", crate::deploy::shlex_quote(command));
        let output = match execute(ssh, &remote_command, open).await {
            Ok(output) => output,
            Err(error) => {
                let sentence = format!("registry authority SSH failed: {error:#}");
                refuse_authority(target, reader, &sentence);
                return Err(AuthorityUnread {
                    sentence,
                    silent: true,
                });
            }
        };
        // SSH carries the exit status as a u32; zero is success, and a channel
        // that closed without one did not succeed and is said as such.
        let status = match output.exit_status {
            Some(code) if std::num::NonZeroU32::new(code).is_none() => None,
            Some(code) => Some(format!("registry authority exited with {code}")),
            None => Some("registry authority closed its channel with no exit status".to_string()),
        };
        if let Some(status) = status {
            let detail = String::from_utf8_lossy(&output.stderr);
            let detail = detail.trim();
            let sentence = if detail.is_empty() {
                status
            } else {
                format!("{status}: {detail}")
            };
            // Only the two transport branches publish. An authority that
            // answers with an unparseable snapshot is reachable and wrong,
            // which is a different finding from a silent host and must not
            // be counted as one.
            refuse_authority(target, reader, &sentence);
            return Err(AuthorityUnread {
                sentence,
                silent: false,
            });
        }
        let answered = |sentence: String| AuthorityUnread {
            sentence,
            silent: false,
        };
        let payload: SnapshotPayload = serde_json::from_slice(&output.stdout)
            .map_err(|error| answered(format!("invalid registry authority response: {error}")))?;
        validate_snapshot(payload).map_err(answered)
    }
}

/// Why a read of the registry authority returned no snapshot, and whether
/// it never reached the host: `silent` when every SSH path failed to
/// connect, which the next reader would stand again; otherwise the host
/// answered, with a refusal or a document that is not a snapshot.
struct AuthorityUnread {
    sentence: String,
    silent: bool,
}

fn parsed_registry(document: &Value) -> Result<targets::Registry, String> {
    let text = serde_json::to_string(document)
        .map_err(|error| format!("the registry document cannot be written as JSON: {error}"))?;
    targets::load_registry_from_str(&text)
        .map_err(|error| format!("the registry document is not a registry: {error}"))
}

pub(crate) fn current_target(document: &Value) -> Result<String, String> {
    let hostname = crate::providers::vast::system_hostname();
    parsed_registry(document)?
        .lookup_self(&hostname)
        .map_err(|error| {
            format!("this host {hostname:?} cannot be found in the registry: {error}")
        })?
        .map(|target| target.name.clone())
        .ok_or_else(|| format!("resolver host {hostname:?} has no registry target identity"))
}

pub(crate) fn snapshot_source(
    local_store: Option<Arc<RegistryStore>>,
    document: &Value,
    local_target: &str,
) -> Result<SnapshotSource, String> {
    let directory = service_resolution::directory(document)?
        .ok_or_else(|| "registry.service_directory is required".to_string())?;
    if directory.authority.target == local_target {
        return local_store
            .map(SnapshotSource::Local)
            .ok_or_else(|| "local registry authority backend is unavailable".to_string());
    }
    let registry = parsed_registry(document)?;
    let target = registry
        .lookup(&directory.authority.target)
        .ok_or_else(|| "registry authority target disappeared".to_string())?;
    let ssh = target_ssh_paths(target);
    if ssh.is_empty() {
        return Err("registry authority has no SSH connection path".to_string());
    }
    Ok(SnapshotSource::Authority {
        target: target.name.clone(),
        ssh,
        command: directory.authority.command,
    })
}
