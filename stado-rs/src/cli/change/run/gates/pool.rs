//! The worker pool a candidate's compiling gates run on: per declared
//! platform, the registry hosts of that release platform. Named hosts
//! (`stado change run --host`) are the pool as named, the operator's own
//! placement, as `stado host cargo HOST` is. Otherwise the pool is read
//! from each host's capacity publication (`placement::hosts::judge`, the
//! placed quality check's judgement): admitted and waiting hosts take
//! shards, refused ones do not, and every verdict is said.

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
        for verdict in hosts::judge(&registry, targets, WORK).await? {
            said.push(verdict.describe());
            if !matches!(verdict.standing, Standing::Refused(_)) {
                by_platform
                    .entry(verdict.target.release_platform.clone())
                    .or_default()
                    .push(verdict.target);
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
