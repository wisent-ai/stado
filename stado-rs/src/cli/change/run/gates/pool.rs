//! The worker pool a candidate's compiling gates run on: per declared
//! platform, the registry hosts of that release platform. Named hosts
//! (`stado change run --host`) are the pool as named, the operator's own
//! placement, as `stado host cargo HOST` is. Otherwise the pool is read
//! from each host's capacity publication (`placement::hosts::judge`, the
//! placed quality check's judgement): admitted and waiting hosts take
//! shards, refused ones do not, and every verdict is said. When the fleet
//! store the publications are kept in cannot be read, the pool is this
//! host first and every other host the registry names as it stands,
//! unjudged, said with the store's error: the queue that lands a repair of
//! the fleet store must not wait on that store, and a host that cannot run
//! its shard (its disk at the threshold, unreachable) is left out of the
//! pass as any host is.

use std::collections::{BTreeMap, BTreeSet};

use crate::cli::CmdError;
use crate::placement::hosts::{self, Standing};
use crate::targets::ComputeTarget;

/// The placed work a host publishes it runs (`placed_work`).
const WORK: &str = "quality-check";

pub struct Pool {
    pub by_platform: BTreeMap<String, Vec<ComputeTarget>>,
    /// One line per host: where it stands and why.
    pub said: Vec<String>,
}

/// The pool for `platforms`, from `named` hosts or from the placement.
pub async fn pool(named: &[String], platforms: &BTreeSet<String>) -> Result<Pool, CmdError> {
    let mut by_platform: BTreeMap<String, Vec<ComputeTarget>> = BTreeMap::new();
    let mut said = Vec::new();
    if named.is_empty() {
        let (registry, targets) = hosts::fleet_hosts(None).await?;
        match hosts::judge(&registry, targets.clone(), WORK).await {
            Ok(verdicts) => {
                for verdict in verdicts {
                    said.push(verdict.describe());
                    if !matches!(verdict.standing, Standing::Refused(_)) {
                        by_platform
                            .entry(verdict.target.release_platform.clone())
                            .or_default()
                            .push(verdict.target);
                    }
                }
            }
            Err(unread) => {
                let (here, others): (Vec<ComputeTarget>, Vec<ComputeTarget>) = targets
                    .into_iter()
                    .partition(crate::deploy::host_channel::target_is_this_host);
                for target in here {
                    said.push(format!(
                        "{} ({}) is this host and runs the gates of its platform first, without \
                         the fleet's placement, since the hosts' capacity publications could not \
                         be read: {unread}",
                        target.name, target.release_platform
                    ));
                    by_platform
                        .entry(target.release_platform.clone())
                        .or_default()
                        .push(target);
                }
                for target in others {
                    said.push(format!(
                        "{} ({}) runs gates of its platform unjudged, since the hosts' capacity \
                         publications could not be read",
                        target.name, target.release_platform
                    ));
                    by_platform
                        .entry(target.release_platform.clone())
                        .or_default()
                        .push(target);
                }
                if by_platform.is_empty() {
                    return Err(unread.also(
                        "and the registry names no host this machine can reach in the fleet's \
                         place; name hosts with --host, or let the fleet store answer again",
                    ));
                }
            }
        }
    } else {
        for name in named {
            let target = crate::cli::canonical_host(name).await?;
            said.push(format!(
                "{} named, {}",
                target.name, target.release_platform
            ));
            by_platform
                .entry(target.release_platform.clone())
                .or_default()
                .push(target);
        }
    }
    let missing: Vec<&String> = platforms
        .iter()
        .filter(|platform| by_platform.get(*platform).is_none_or(Vec::is_empty))
        .collect();
    if !missing.is_empty() {
        let hosts = if said.is_empty() {
            "the registry declares no reachable host".to_string()
        } else {
            said.join("; ")
        };
        return Err(CmdError::unreachable(format!(
            "no fleet host of {} can run the candidate's compiling gates: {hosts}; name hosts \
             with --host, or bring a host of that platform back to publishing capacity",
            missing
                .iter()
                .map(|platform| platform.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    Ok(Pool { by_platform, said })
}
