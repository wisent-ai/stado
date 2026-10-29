//! The compiling shards on the pool: one worker per host, each taking the
//! next shard of its platform until none is left, all hosts at once. A
//! shard the action cache answers takes no host, and a candidate the cache
//! answers whole asks for no pool. A refused shard stops the hosts taking
//! new ones, since the candidate is refused whatever the rest say; a host
//! that cannot run a shard at all is dropped and its shard given back to
//! the platform's other hosts, which take another round when they had
//! already ended.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use super::pool::pool;
use super::shard::Work;
use super::{cache, tail, Plan, Planned};
use crate::cli::change::record::Shard;
use crate::cli::CmdError;
use crate::deploy::host_run::HostCargoRequest;
use crate::targets::ComputeTarget;

struct Shared {
    waiting: Mutex<BTreeMap<String, VecDeque<Planned>>>,
    done: Mutex<Vec<Shard>>,
    /// Each host that could not run its shard, and the sentence saying so.
    lost: Mutex<Vec<(String, String)>>,
    refused: AtomicBool,
    /// The version a declared gate is told the release would cut.
    version: String,
    /// The history records a release build's archive carries beside the
    /// commit's files, given to every declared gate.
    records: Vec<(&'static str, Vec<u8>)>,
}

fn poisoned<T>(_: T) -> CmdError {
    CmdError::click("a gate worker panicked")
}

/// Every shard of `plan` over `commit` of `checkout`, run or answered, on
/// the pool of the `hosts` named (`stado change run --host`), else of the
/// placement.
pub async fn run(
    checkout: &Path,
    commit: &str,
    tree: &str,
    plan: &Plan,
    hosts: &[String],
) -> Result<Vec<Shard>, CmdError> {
    let mut waiting: BTreeMap<String, VecDeque<Planned>> = BTreeMap::new();
    let mut done = Vec::new();
    for planned in &plan.compiling {
        let key = cache::key(tree, &planned.platform, &planned.gate, &planned.argv());
        match cache::hit(&key)? {
            Some(hit) => done.push(hit),
            None => waiting
                .entry(planned.platform.clone())
                .or_default()
                .push_back(planned.clone()),
        }
    }
    for shard in &done {
        eprintln!(
            "stado change run: {} for {}: answered by the action cache (`{}` passed on {})",
            shard.gate,
            shard.platform,
            shard.argv.join(" "),
            shard.host
        );
    }
    if waiting.is_empty() {
        return Ok(done);
    }
    let platforms: BTreeSet<String> = waiting.keys().cloned().collect();
    let placed = pool(hosts, &platforms).await?;
    for line in &placed.said {
        eprintln!("stado change run: placement: {line}");
    }
    // A declared gate reads the source a release build reads, history
    // records included (a version gate checks its baseline's provenance).
    let declared = waiting
        .values()
        .flatten()
        .any(|planned| matches!(planned.work, Work::Declared(_)));
    let records = if declared {
        crate::cli::release_submit::history_records(checkout, commit)?
    } else {
        Vec::new()
    };
    let shared = Shared {
        waiting: Mutex::new(waiting),
        done: Mutex::new(done),
        lost: Mutex::new(Vec::new()),
        refused: AtomicBool::new(false),
        version: plan.version.clone(),
        records,
    };
    let mut workers: Vec<(&str, &ComputeTarget)> = placed
        .by_platform
        .iter()
        .flat_map(|(platform, targets)| targets.iter().map(move |host| (platform.as_str(), host)))
        .collect();
    // A host that could not run its shard gave it back; the platform's
    // other hosts may have ended meanwhile, so they take another round
    // while a shard of their platform waits.
    while !workers.is_empty() {
        let round = workers
            .iter()
            .map(|(platform, host)| worker(checkout, commit, tree, platform, host, &shared));
        futures::future::join_all(round).await;
        let lost = shared.lost.lock().map_err(poisoned)?;
        workers.retain(|(_, host)| !lost.iter().any(|(name, _)| *name == host.name));
        drop(lost);
        let waiting = shared.waiting.lock().map_err(poisoned)?;
        let more = workers.iter().any(|(platform, _)| {
            waiting
                .get(*platform)
                .is_some_and(|queue| !queue.is_empty())
        });
        drop(waiting);
        if !more || shared.refused.load(Ordering::SeqCst) {
            break;
        }
    }
    let left: usize = shared
        .waiting
        .into_inner()
        .map_err(poisoned)?
        .values()
        .map(VecDeque::len)
        .sum();
    let lost = shared.lost.into_inner().map_err(poisoned)?;
    let done = shared.done.into_inner().map_err(poisoned)?;
    if left > 0 && !shared.refused.load(Ordering::SeqCst) {
        return Err(CmdError::unreachable(format!(
            "{left} gate shard(s) were left with no host to run them: {}",
            lost.iter()
                .map(|(_, why)| why.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }
    Ok(done)
}

/// The next shard of `platform`, unless a shard was refused.
fn next(shared: &Shared, platform: &str) -> Option<Planned> {
    if shared.refused.load(Ordering::SeqCst) {
        return None;
    }
    shared.waiting.lock().ok()?.get_mut(platform)?.pop_front()
}

async fn worker(
    checkout: &Path,
    commit: &str,
    tree: &str,
    platform: &str,
    host: &ComputeTarget,
    shared: &Shared,
) {
    let runner = crate::deploy::production_runner();
    while let Some(planned) = next(shared, platform) {
        let argv = planned.argv();
        eprintln!(
            "stado change run: {} for {platform} on {}: `{}`",
            planned.gate,
            host.name,
            argv.join(" ")
        );
        let started = Instant::now();
        let ran = match &planned.work {
            Work::Declared(declared) => crate::deploy::host_run::gate(
                host,
                &checkout.join(&planned.manifest),
                commit,
                (declared, shared.version.as_str(), platform),
                &shared.records,
                &runner,
            )
            .await
            .map(|receipt| (receipt.exit_code, receipt.stdout, receipt.stderr)),
            Work::Cargo {
                operation,
                binaries,
                arguments,
            } => {
                let request = HostCargoRequest {
                    manifest_path: checkout.join(&planned.manifest),
                    revision: commit.to_string(),
                    operation: *operation,
                    binaries: binaries.clone(),
                    arguments: arguments.clone(),
                };
                crate::deploy::host_run::cargo(host, &request, &runner)
                    .await
                    .map(|receipt| (receipt.exit_code, receipt.stdout, receipt.stderr))
            }
        };
        let (exit_code, stdout, stderr) = match ran {
            Ok(ran) => ran,
            Err(error)
                if matches!(
                    error.failure,
                    Some(
                        crate::primitives::failure::FailureCode::Refused
                            | crate::primitives::failure::FailureCode::Config
                    )
                ) =>
            {
                // The request itself was refused before any host ran it (the
                // commit's manifest, its source): the candidate's fault, not
                // the host's, so it refuses the candidate and keeps the host.
                shared.refused.store(true, Ordering::SeqCst);
                if let Ok(mut done) = shared.done.lock() {
                    done.push(Shard {
                        gate: planned.gate.clone(),
                        kind: planned.kind.to_string(),
                        platform: platform.to_string(),
                        argv: argv.clone(),
                        host: host.name.clone(),
                        cached: false,
                        exit_code: -1,
                        seconds: started.elapsed().as_secs(),
                        report: format!("not run: {error}"),
                    });
                }
                return;
            }
            Err(error) => {
                if let Ok(mut lost) = shared.lost.lock() {
                    lost.push((
                        host.name.clone(),
                        format!("{} could not run {}: {error}", host.name, planned.gate),
                    ));
                }
                if let Ok(mut waiting) = shared.waiting.lock() {
                    waiting
                        .entry(platform.to_string())
                        .or_default()
                        .push_front(planned);
                }
                eprintln!(
                    "stado change run: {} is left out of this pass: {error}",
                    host.name
                );
                return;
            }
        };
        let shard = Shard {
            gate: planned.gate.clone(),
            kind: planned.kind.to_string(),
            platform: platform.to_string(),
            argv: argv.clone(),
            host: host.name.clone(),
            cached: false,
            exit_code,
            seconds: started.elapsed().as_secs(),
            report: tail(&format!("{stdout}\n{stderr}")),
        };
        eprintln!(
            "stado change run: {} for {platform} on {} exited {} after {}s",
            planned.gate, host.name, shard.exit_code, shard.seconds
        );
        if shard.exit_code != 0 {
            shared.refused.store(true, Ordering::SeqCst);
        } else if let Err(error) =
            cache::keep(&cache::key(tree, platform, &planned.gate, &argv), &shard)
        {
            eprintln!(
                "stado change run: the pass of {} was not kept in the action cache: {error}",
                planned.gate
            );
        }
        if let Ok(mut done) = shared.done.lock() {
            done.push(shard);
        }
    }
}
