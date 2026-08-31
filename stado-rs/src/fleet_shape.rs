//! Standing checks for the shape of the fleet: is what is declared what is
//! running, and does anything measure the difference.
//!
//! NO Python original. Written on 2026-08-31 after a night in which seven
//! defects of ONE shape were fixed by hand and nothing in the product would
//! have caught the eighth. Every check here is a question somebody had to ask
//! a host by hand that night, and the answer each time was a surprise:
//!
//! - three processes served one declared port on `charless-mac-mini`
//!   (`127.0.0.1:8765`, `[::1]:8765`, and a `node` on the tailnet address),
//!   found with `lsof` after hours of treating the symptom as a slow link;
//! - a label declared in two launchd domains ran twice and was invisible to
//!   `service list --undeclared`, precisely BECAUSE the label was declared;
//! - the live object API answered `healthz` 200 while every object route
//!   returned 503, so the health check was green on a server refusing its
//!   entire purpose;
//! - a primary addressed by bare key with a replica addressed by qualified
//!   path silently produced 48 GiB of objects nothing could resolve;
//! - a managed host declared two cleaners, neither of which could reach what
//!   actually filled its disk, and nothing said so.
//!
//! The rule this module exists to enforce on itself: **a check that cannot
//! fail is the disease.** So every check reports what it MEASURED, and a check
//! that could not measure its subject says that in a finding rather than
//! passing quietly. `measured` on [`Sweep`] is the count of subjects actually
//! interrogated, and a sweep that measured nothing is not a clean sweep.
//!
//! Each finding names four things, because a verdict without them is what made
//! these take hours: the SUBJECT it is about, what the fleet DECLARES, what was
//! OBSERVED, and the exact COMMAND that resolves it.
//!
//! Two entry points, one implementation: [`sweep`] is called by
//! [`crate::doctor`] for an operator asking now, and by
//! [`crate::coordinator`]'s tick so nobody has to ask. The tick is the reason
//! this is not another command nobody runs.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};

use crate::deploy::{host_channel, host_disk, service, Runner};
use crate::queue::copy::Endpoint;
use crate::targets::{ComputeTarget, Registry};

/// One thing that is not the way the fleet says it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Stable id of the check that produced it, for grepping a tick log.
    pub check: &'static str,
    /// What the finding is about: a host, a label, a port, a store.
    pub subject: String,
    /// What the fleet declares about that subject.
    pub declared: String,
    /// What was actually observed.
    pub observed: String,
    /// The exact command that resolves it.
    pub command: String,
}

impl Finding {
    pub fn to_json(&self) -> Value {
        json!({
            "check": self.check,
            "subject": self.subject,
            "declared": self.declared,
            "observed": self.observed,
            "command": self.command,
        })
    }

    /// One line carrying all four parts. The tick log is the only place some
    /// of these will ever be read, so the line has to be the whole finding.
    pub fn line(&self) -> String {
        format!(
            "{}: {} — declared {} — observed {} — fix: {}",
            self.check, self.subject, self.declared, self.observed, self.command
        )
    }
}

/// What one sweep looked at and what it found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sweep {
    pub findings: Vec<Finding>,
    /// Subjects actually interrogated. A sweep with zero of these has proven
    /// nothing, and says so instead of reading as healthy.
    pub measured: u32,
    /// Hosts the sweep could not reach at all, by name and reason.
    pub unreachable: Vec<(String, String)>,
    /// What a check measured when it had nothing to report. Present so that a
    /// silent check and a check with nothing to check are distinguishable.
    pub notes: Vec<String>,
}

impl Sweep {
    fn record(&mut self, finding: Finding) {
        self.findings.push(finding);
    }

    pub fn summary(&self) -> String {
        if self.measured == 0 {
            return format!(
                "fleet shape: NOTHING measured ({} host(s) unreachable), so this is not a clean result",
                self.unreachable.len()
            );
        }
        format!(
            "fleet shape: {} subject(s) measured, {} finding(s), {} host(s) unreachable{}",
            self.measured,
            self.findings.len(),
            self.unreachable.len(),
            if self.notes.is_empty() {
                String::new()
            } else {
                format!(" — measured clean: {}", self.notes.join(", "))
            }
        )
    }
}

/// Per-host wall clock. A host that has gone quiet must cost one line, not the tick. Three remote
/// tick it was swept from.
const HOST_TIMEOUT: Duration = Duration::from_secs(240);

pub const PORT_CHECK: &str = "one-listener-per-declared-port";
pub const DOMAIN_CHECK: &str = "one-domain-per-declared-label";
pub const HEALTH_CHECK: &str = "health-green-boundaries-down";
pub const REPLICA_CHECK: &str = "replica-cannot-resolve";
pub const DISK_CHECK: &str = "disk-headroom-against-policy";

/// Sweep the whole canonical registry.
///
/// Never returns an error: an unreachable host is a recorded fact, because the
/// useful output is the whole list and one dead box must not suppress it.
pub async fn sweep(runner: &Runner) -> Sweep {
    let mut result = Sweep::default();
    let registry = match host_channel::canonical_registry().await {
        Ok(registry) => registry,
        Err(error) => {
            result
                .unreachable
                .push(("<registry>".to_string(), error.to_string()));
            return result;
        }
    };
    // The store-addressing check is answered from configuration alone, so it
    // runs even when every host is unreachable.
    replica_addressing(&mut result);
    for target in registry.targets.iter().filter(|target| target.slots > 0) {
        sweep_host(&registry, target, runner, &mut result).await;
    }
    result
}

/// Everything one host is asked, under one deadline.
async fn sweep_host(
    registry: &Registry,
    target: &ComputeTarget,
    runner: &Runner,
    result: &mut Sweep,
) {
    match tokio::time::timeout(HOST_TIMEOUT, host_findings(registry, target, runner)).await {
        Ok(Ok(mut findings)) => {
            result.measured += 1;
            for finding in findings.drain(..) {
                result.record(finding);
            }
        }
        Ok(Err(error)) => result.unreachable.push((target.name.clone(), error)),
        Err(_) => result.unreachable.push((
            target.name.clone(),
            format!("did not answer within {}s", HOST_TIMEOUT.as_secs()),
        )),
    }
}

async fn host_findings(
    registry: &Registry,
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<Vec<Finding>, String> {
    let mut findings = Vec::new();
    let loaded = service::loaded_units(target, runner)
        .await
        .map_err(|error| error.to_string())?;
    duplicate_domains(target, &loaded, &mut findings);
    listener_count(registry, target, runner, &mut findings).await;
    disk_headroom(target, runner, &mut findings).await;
    Ok(findings)
}

/// One label, one declaring domain.
///
/// The launchd domains are read by
/// [`crate::deploy::service::loaded_units`], which reports every unit file it
/// found for a label rather than the first — the change that made this
/// detectable at all.
fn duplicate_domains(
    target: &ComputeTarget,
    loaded: &[service::UndeclaredUnit],
    out: &mut Vec<Finding>,
) {
    for unit in loaded {
        if unit.declaring_paths.len() < 2 {
            continue;
        }
        out.push(Finding {
            check: DOMAIN_CHECK,
            subject: format!("{}:{}", target.name, unit.label),
            declared: "one unit file per label".to_string(),
            observed: format!(
                "{} unit files declare it: {}",
                unit.declaring_paths.len(),
                unit.declaring_paths.join(", ")
            ),
            command: format!(
                "stado host remove-file {} <the domain that should not own it> then stado service ensure {}",
                target.name, unit.label
            ),
        });
    }
}

/// One process per declared service port, and one health verdict that agrees
/// with the routes behind it.
///
/// Both come out of the same inventory read: it reports the loopback listeners
/// a host holds and the service directory it is supposed to satisfy.
async fn listener_count(
    registry: &Registry,
    target: &ComputeTarget,
    runner: &Runner,
    out: &mut Vec<Finding>,
) {
    let reading = match crate::deploy::host_inventory::inventory_target(
        target,
        registry.service_directory.as_ref(),
        runner,
    )
    .await
    {
        Ok(reading) => reading,
        Err(error) => {
            out.push(Finding {
                check: PORT_CHECK,
                subject: target.name.clone(),
                declared: "the host answers a listener inventory".to_string(),
                observed: format!("inventory failed: {error}"),
                command: format!("stado host inventory {}", target.name),
            });
            return;
        }
    };
    let listeners = reading
        .get("listeners")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let state = reading
        .get("listeners_state")
        .and_then(Value::as_str)
        .unwrap_or("");
    // A table nobody could read is not an empty table. Without this the check
    // would report "one listener per port" on a host whose `lsof` failed,
    // which is the exact false pass this module exists to refuse.
    if listeners.is_empty() && state != crate::deploy::host_inventory::LISTENERS_READ {
        out.push(Finding {
            check: PORT_CHECK,
            subject: target.name.clone(),
            declared: "the host reports its listeners".to_string(),
            observed: format!("listener table unread ({state})"),
            command: format!("stado host inventory {}", target.name),
        });
        return;
    }
    let mut holders: BTreeMap<u64, Vec<String>> = BTreeMap::new();
    for listener in &listeners {
        let Some(port) = listener.get("port").and_then(Value::as_u64) else {
            continue;
        };
        let who = format!(
            "{}:{port} pid {}",
            listener
                .get("address")
                .and_then(Value::as_str)
                .unwrap_or("?"),
            listener.get("pid").and_then(Value::as_u64).unwrap_or(0),
        );
        holders.entry(port).or_default().push(who);
    }
    for (port, who) in holders {
        if who.len() < 2 {
            continue;
        }
        out.push(Finding {
            check: PORT_CHECK,
            subject: format!("{}:{port}", target.name),
            declared: "one process serves one declared port".to_string(),
            observed: format!("{} processes hold it: {}", who.len(), who.join(" | ")),
            command: format!(
                "stado service list --undeclared --host {0} and stado service reap --host {0} --command <substring> (report first, then --apply)",
                target.name
            ),
        });
    }
}

/// Free space against the watermark the registry declares, and a finding when
/// a managed host declares no policy at all.
async fn disk_headroom(target: &ComputeTarget, runner: &Runner, out: &mut Vec<Finding>) {
    let report = match host_disk::disk_host(&target.name, runner).await {
        Ok(report) => report,
        Err(error) => {
            out.push(Finding {
                check: DISK_CHECK,
                subject: target.name.clone(),
                declared: "the host answers df".to_string(),
                observed: format!("disk read failed: {error}"),
                command: format!("stado host disk {}", target.name),
            });
            return;
        }
    };
    let Some(policy) = target.disk_cleanup.as_ref() else {
        out.push(Finding {
            check: DISK_CHECK,
            subject: target.name.clone(),
            declared: "no disk_cleanup policy".to_string(),
            observed: "a managed host with slots and no watermark, so nothing on it is ever \
                       obliged to free space"
                .to_string(),
            command: format!(
                "add targets[{}].disk_cleanup to the registry, then stado registry validate and push",
                target.name
            ),
        });
        return;
    };
    let available_kb = report
        .get("usage")
        .and_then(|usage| usage.get("available_kb"))
        .and_then(Value::as_str)
        .and_then(|value| value.trim().parse::<f64>().ok());
    let Some(available_kb) = available_kb else {
        out.push(Finding {
            check: DISK_CHECK,
            subject: target.name.clone(),
            declared: format!("low watermark {} GiB", policy.low_free_gb),
            observed: "df answered without an available column".to_string(),
            command: format!("stado host disk {} --json", target.name),
        });
        return;
    };
    let free_gib = host_disk::gib_from_blocks(available_kb);
    if free_gib < policy.low_free_gb as f64 {
        out.push(Finding {
            check: DISK_CHECK,
            subject: target.name.clone(),
            declared: format!(
                "low watermark {} GiB, target {} GiB, mode {}",
                policy.low_free_gb, policy.target_free_gb, policy.mode
            ),
            observed: format!("{free_gib:.1} GiB free, so this host is refusing work"),
            command: format!(
                "stado host reclaim {} --apply --reason <why> and stado host backup-audit {} --reclaim-twins --apply",
                target.name, target.name
            ),
        });
    }
    // A cleaner set that cannot reach what fills the machine is the defect the
    // janitor spent a week not fixing on the always-on mac: it declared
    // huggingface_cache and weles_recordings while cargo build trees and a
    // same-disk replica took the disk down.
    let declared_cleaners: Vec<&str> = policy.cleaners.keys().map(String::as_str).collect();
    for expected in ["queue_workdirs", "backup_twins"] {
        if !declared_cleaners.contains(&expected) {
            out.push(Finding {
                check: DISK_CHECK,
                subject: format!("{}:{expected}", target.name),
                declared: format!("cleaners {}", declared_cleaners.join(", ")),
                observed: format!("{expected} is not declared, so the janitor cannot reclaim what it owns"),
                command: format!(
                    "add {expected} to targets[{}].disk_cleanup.cleaners AFTER the host runs a binary that knows it",
                    target.name
                ),
            });
        }
    }
}

/// A replica that can never hold what the primary writes.
///
/// [`Endpoint::cannot_replicate`] is the same predicate the write path and the
/// coordinator's replication both consult; this reports the condition standing
/// rather than waiting for someone to notice 48 GiB of unresolvable objects.
///
/// **Reach: THIS control plane's configuration only.** The pairing that
/// actually produced 48 GiB of unaddressable objects on 2026-08-30 was
/// `charless-mac-mini`'s own — `wc_storage_backend: stado` with
/// `wc_backup_storage_backend: local`, read from that host's config, not from
/// here. This control plane declares `storage.backup: null` and so has nothing
/// to disagree about, which is why this arm reports a note rather than a
/// finding on the fleet it was written for. Extending it means reading each
/// host's resolved config the way `stado host config-show` does, one call per
/// host, and that is the next thing this check needs.
fn replica_addressing(result: &mut Sweep) {
    let primary = Endpoint::configured_primary();
    result.measured += 1;
    // Which of the three states this control plane is in is recorded either
    // way. A check whose quiet result and whose "there was nothing to check"
    // result look identical is the shape this module exists to refuse: on the
    // first live sweep this arm produced no finding and no note, and there was
    // no way to tell from the output whether the pairing was sound or simply
    // never read.
    // What the config FILE declares, beside what the resolver answers. These
    // disagreed on this control plane on 2026-08-31: the file declares
    // `storage.backup.backend = local` with a path, `stado config show`
    // resolves `wc_backup_storage_backend` to empty, `stado doctor`'s backup
    // row passes with "no mandatory S3 replica" — and a `storage ls` in the
    // same worktree printed the mirror refusal naming
    // `local://~/.stado/local-backup`, which requires that key to be set.
    // Two readers, two answers, one declaration: components that believe the
    // replica exists write 48 GiB into it while the diagnostics say there is
    // nothing there and pass.
    let declared_in_file = crate::config_file::load_config_file()
        .ok()
        .and_then(|file| file.get("storage").cloned())
        .and_then(|storage| storage.get("backup").cloned())
        .and_then(|backup| {
            backup
                .get("backend")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|backend| !backend.trim().is_empty());
    let Some(backup) = Endpoint::configured_backup() else {
        match declared_in_file {
            Some(backend) => result.record(Finding {
                check: REPLICA_CHECK,
                subject: format!("{}: storage.backup.backend", primary.describe()),
                declared: format!("the config file declares a {backend} replica"),
                observed: "the resolver answers that no replica is configured, so one half of \
                           this binary writes to a replica the other half says does not exist"
                    .to_string(),
                command: "compare `stado config show` against storage.backup in the config file; \
                          the resolver is the half to fix"
                    .to_string(),
            }),
            None => result
                .notes
                .push(format!("{}: no replica declared", primary.describe())),
        }
        return;
    };
    match primary.cannot_replicate(&backup) {
        Some(refusal) => result.record(Finding {
            check: REPLICA_CHECK,
            subject: format!("{} -> {}", primary.describe(), backup.describe()),
            declared: "storage.backup is a disaster-recovery replica of storage".to_string(),
            observed: refusal,
            command: "stado config set storage.backup.backend \"\" to stop declaring a replica, \
                      or point it at a store of the same kind as the primary"
                .to_string(),
        }),
        None => result.notes.push(format!(
            "{} can replicate to {}",
            primary.describe(),
            backup.describe()
        )),
    }
}

/// Whether a service that reports itself healthy is actually serving.
///
/// Read from the endpoint this control plane is configured to use, which is the
/// one whose answers the fleet depends on. `healthz` answering 200 while its
/// own boundaries are closed is not a healthy service: every authorized route
/// behind it returns 503, which is how a store outage read as a slow link for
/// most of a night.
pub async fn health_disagreement() -> Option<Finding> {
    let url = crate::config::wc_stado_storage_url();
    if url.is_empty() {
        return None;
    }
    let endpoint = format!("{}/healthz", url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(u8::BITS as u64))
        .build()
        .ok()?;
    let body: Value = client.get(&endpoint).send().await.ok()?.json().await.ok()?;
    let ok = body.get("ok").and_then(Value::as_bool).unwrap_or_default();
    let degraded = body
        .get("degraded")
        .and_then(Value::as_bool)
        .unwrap_or_default();
    let closed: Vec<String> = body
        .get("boundaries")
        .and_then(Value::as_object)
        .map(|boundaries| {
            boundaries
                .iter()
                .filter(|(_, open)| open.as_bool() == Some(false))
                .map(|(name, _)| name.clone())
                .collect()
        })
        .unwrap_or_default();
    if !ok || !degraded || closed.is_empty() {
        return None;
    }
    Some(Finding {
        check: HEALTH_CHECK,
        subject: endpoint,
        declared: "healthz reports whether the service can do its work".to_string(),
        observed: format!(
            "healthz says ok while {} boundary/boundaries are closed ({}), so every route behind \
             them answers 503",
            closed.len(),
            closed.join(", ")
        ),
        command: "stado service logs com.wisent.always-on.stado-object-api --host <host> names \
                  why the boundary is closed; a credential answer is not fixed by restarting the \
                  process"
            .to_string(),
    })
}
