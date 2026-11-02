//! `stado capabilities resolve FAMILY --consumer C`: the first alternative of
//! a family, in the selected order, that is proven to answer, and every one
//! before it that did not; a typed refusal naming each alternative's product,
//! route and cause when none answers.
//!
//! A proof is made only when the kept one is older than the declared
//! interval (the refresh interval this host's resolver declares), or when
//! the caller reports with `--failed` that the alternative it was handed
//! refused it. Each answer records which alternative served the consumer
//! ([`super::record`]), so a fallback in use is read in `stado capabilities`.

use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Value};

use super::record::{self, Proof, Served, Unreachable};
use crate::capabilities::alternatives::{Alternative, Family, Reach};
use crate::cli::CmdError;

/// What one alternative answered when it was proven now.
struct Proven {
    route: String,
    outcome: Result<(), String>,
}

/// The time a proof stands: the directory refresh interval this host's
/// resolver declares, the one measure of a wait that stands. `Err` names why
/// it cannot be read; `Ok(None)` is a host that declares no resolver, where
/// every resolve proves again.
fn interval() -> (Option<Duration>, Option<String>) {
    match crate::cli::resolver::declared_bound_seconds() {
        Ok(seconds) => (seconds.map(Duration::from_secs), None),
        Err(cause) => (None, Some(cause)),
    }
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Whether the proof made at `at` still stands under `interval`.
fn stands(at: &str, interval: Option<Duration>) -> bool {
    let (Some(interval), Ok(at)) = (interval, DateTime::parse_from_rfc3339(at)) else {
        return false;
    };
    Utc::now()
        .signed_duration_since(at.with_timezone(&Utc))
        .to_std()
        .is_ok_and(|age| age < interval)
}

/// The selected order of `family`: `capabilities.<family>.order`, else the
/// declared order.
pub(super) fn order(family: &Family) -> Result<Vec<&'static Alternative>, CmdError> {
    let selected: Vec<String> = match crate::config_file::field_value(&family.order) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Some(Value::String(text)) => text.split(',').map(str::to_owned).collect(),
        Some(other) => {
            return Err(CmdError::declaration(format!(
                "{} is {other}, not a list of {} alternatives",
                family.order.path, family.id
            )))
        }
        None => Vec::new(),
    };
    family
        .ordered(&selected)
        .map_err(|cause| CmdError::declaration(format!("{}: {cause}", family.order.path)))
}

/// Run `self_exe service directory connect SERVICE --consumer CONSUMER --json`
/// to its end or until `bound`, and read the route it answered.
async fn directory(service: &str, consumer: &str, bound: Option<Duration>) -> Proven {
    let route = format!("service directory route {service} for {consumer}");
    let program = match std::env::current_exe() {
        Ok(program) => program,
        Err(error) => {
            return Proven {
                route,
                outcome: Err(format!("this Stado cannot name its own program: {error}")),
            }
        }
    };
    let mut command = tokio::process::Command::new(program);
    command
        .args([
            "service",
            "directory",
            "connect",
            service,
            "--consumer",
            consumer,
            "--json",
        ])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let run = async {
        match bound {
            Some(bound) => tokio::time::timeout(bound, command.output())
                .await
                .unwrap_or_else(|_| {
                    Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        format!(
                            "no answer within the {}s this host's resolver declares as its refresh interval",
                            bound.as_secs()
                        ),
                    ))
                }),
            None => command.output().await,
        }
    };
    let output = crate::wait::until(
        crate::wait::Kind::Process,
        format!("stado service directory connect {service} --consumer {consumer}"),
        "this host's resolver",
        run,
    )
    .await;
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            return Proven {
                route,
                outcome: Err(error.to_string()),
            }
        }
    };
    if !output.status.success() {
        // The child answers `--json`, so its refusal is the envelope on
        // stdout and stderr carries only what it logged; a child that died
        // before printing one says why on stderr.
        let said = String::from_utf8_lossy(&output.stderr);
        let cause = serde_json::from_slice::<Value>(&output.stdout)
            .ok()
            .and_then(|envelope| envelope["message"].as_str().map(str::to_owned))
            .or_else(|| {
                said.lines()
                    .rev()
                    .find_map(|line| line.strip_prefix("Error: "))
                    .map(str::to_owned)
            })
            .or_else(|| {
                said.lines()
                    .rev()
                    .find(|line| !line.trim().is_empty())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "it exited without saying why".to_owned());
        return Proven {
            route,
            outcome: Err(format!("{cause} ({})", output.status)),
        };
    }
    match serde_json::from_slice::<Value>(&output.stdout)
        .ok()
        .and_then(|answer| answer["url"].as_str().map(str::to_owned))
    {
        Some(url) => Proven {
            route: url,
            outcome: Ok(()),
        },
        None => Proven {
            route,
            outcome: Err("the directory's answer named no url".to_owned()),
        },
    }
}

/// Prove the attestation alternative for `consumer`: its directory has a
/// place. Whether a statement answers a request is read when the request is
/// made (`stado capabilities call`), since a statement answers one request.
fn attestation(family: &Family, consumer: &str) -> Proven {
    match super::attestation::directory(family, consumer) {
        Ok(directory) => Proven {
            outcome: std::fs::create_dir_all(&directory)
                .map_err(|error| format!("cannot create {}: {error}", directory.display())),
            route: directory.display().to_string(),
        },
        Err(cause) => Proven {
            route: format!("attestations of {consumer}"),
            outcome: Err(cause),
        },
    }
}

fn program(name: &str) -> Proven {
    let path = std::env::var_os("PATH");
    match stado_product::common::step_program(name, path.as_deref()) {
        Ok(found) => Proven {
            route: found.display().to_string(),
            outcome: Ok(()),
        },
        Err(error) => Proven {
            route: format!("program {name}"),
            outcome: Err(format!("{error:#}")),
        },
    }
}

fn owner_file() -> Proven {
    let store = crate::config_file::field_value(&crate::capabilities::CREDENTIALS_STORE_CONFIG)
        .and_then(|value| value.as_str().map(str::to_owned));
    let route = crate::capabilities::CREDENTIALS_STORE_CONFIG
        .path
        .to_owned();
    let Some(path) = store
        .as_deref()
        .and_then(|store| store.strip_prefix("file://"))
    else {
        return Proven {
            route,
            outcome: Err(format!(
                "{} names no file://PATH store (it is {})",
                crate::capabilities::CREDENTIALS_STORE_CONFIG.path,
                store.as_deref().unwrap_or("unset")
            )),
        };
    };
    let outcome = match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => Ok(()),
        Ok(_) => Err(format!("{path} is not a file")),
        Err(error) => Err(format!("{path}: {error}")),
    };
    Proven {
        route: format!("file://{path}"),
        outcome,
    }
}

async fn prove(
    family: &Family,
    alternative: &Alternative,
    consumer: &str,
    bound: Option<Duration>,
) -> Proven {
    match alternative.reach {
        Reach::Directory { service } => directory(service, consumer, bound).await,
        Reach::Attestation => attestation(family, consumer),
        Reach::Program { name } => program(name),
        Reach::OwnerFile => owner_file(),
    }
}

fn kind(reach: &Reach) -> &'static str {
    match reach {
        Reach::Directory { .. } => "directory",
        Reach::Attestation => "attestation",
        Reach::Program { .. } => "program",
        Reach::OwnerFile => "owner-file",
    }
}

/// The alternative that answers, where, and whether it was proven now or
/// its kept proof stands.
pub(super) struct Chosen {
    pub(super) index: usize,
    pub(super) alternative: &'static Alternative,
    pub(super) route: String,
    pub(super) proven_at: String,
    pub(super) proven_now: bool,
}

/// What one resolve found: the alternative chosen, every one before it that
/// did not answer, and the order and interval it was judged under.
pub(super) struct Resolution {
    pub(super) family: &'static Family,
    pub(super) consumer: String,
    pub(super) chosen: Chosen,
    pub(super) skipped: Vec<Unreachable>,
    pub(super) order: Vec<&'static str>,
    pub(super) interval: Option<Duration>,
    pub(super) interval_unread: Option<String>,
}

impl Resolution {
    /// The answer `resolve --json` prints.
    pub(super) fn answer(&self) -> Value {
        json!({
            "family": self.family.id.as_str(),
            "consumer": self.consumer,
            "provider": self.chosen.alternative.provider.as_str(),
            "kind": kind(&self.chosen.alternative.reach),
            "route": self.chosen.route,
            "proven_at": self.chosen.proven_at,
            "proven_now": self.chosen.proven_now,
            "fallback": self.chosen.index > 0,
            "order": self.order,
            "unreachable": unreachable(&self.skipped),
            "interval_seconds": self.interval.map(|interval| interval.as_secs()),
            "interval_unread": self.interval_unread,
        })
    }
}

fn unreachable(skipped: &[Unreachable]) -> Vec<Value> {
    skipped
        .iter()
        .map(|item| json!({"product": item.product, "route": item.route, "cause": item.cause}))
        .collect()
}

/// The refusal when no alternative of `family` answers `consumer`: the
/// sentence names each one's product, route and cause, and the details
/// carry them as data.
pub(super) fn none_answers(
    family: &Family,
    consumer: &str,
    order: &[&str],
    skipped: &[Unreachable],
) -> CmdError {
    let sentence = skipped
        .iter()
        .map(|item| format!("{} at {}: {}", item.product, item.route, item.cause))
        .collect::<Vec<_>>()
        .join("; ");
    CmdError::unreachable(format!(
        "no {} alternative answers {consumer}: {sentence}",
        family.id
    ))
    .detailing(json!({
        "family": family.id.as_str(),
        "consumer": consumer,
        "order": order,
        "unreachable": unreachable(skipped),
    }))
}

/// The first alternative of `family`, in the selected order, proven to
/// answer `consumer`, skipping `excluded` (alternatives the caller already
/// tried for this request), and every one before it that did not; the
/// proofs made and what served the consumer are recorded.
pub(super) async fn choose(
    family: &'static Family,
    consumer: &str,
    failed: &[String],
    excluded: &[Unreachable],
) -> Result<Resolution, CmdError> {
    if consumer.trim().is_empty() {
        return Err(CmdError::usage("--consumer names the product asking"));
    }
    for name in failed {
        if !family
            .alternatives
            .iter()
            .any(|alternative| alternative.provider.matches(name))
        {
            return Err(CmdError::usage(format!(
                "--failed {name}: {} has no alternative {name}; its alternatives are [{}]",
                family.id,
                family.names().join(", ")
            )));
        }
    }
    let ordered = order(family)?;
    let (interval, interval_unread) = interval();
    let kept = record::read()?
        .families
        .remove(family.id.as_str())
        .unwrap_or_default();

    let mut proofs: Vec<(String, Proof)> = Vec::new();
    let mut skipped: Vec<Unreachable> = Vec::new();
    let mut chosen: Option<Chosen> = None;
    for (index, alternative) in ordered.iter().copied().enumerate() {
        let name = alternative.provider.as_str();
        // One the caller already asked for this request is named where it
        // stands in the order, with the cause it gave.
        if let Some(item) = excluded.iter().find(|item| item.product == name) {
            skipped.push(item.clone());
            continue;
        }
        // A directory route is admitted and placed per consumer, so its
        // proof speaks for that consumer alone, as the attestations kept for
        // a consumer do; a program and the owner file answer every consumer
        // alike.
        let key = match alternative.reach {
            Reach::Directory { .. } | Reach::Attestation => format!("{name} for {consumer}"),
            _ => name.to_owned(),
        };
        let reported = failed
            .iter()
            .any(|named| alternative.provider.matches(named));
        let prior = kept
            .proven
            .get(&key)
            .filter(|proof| !reported && stands(&proof.at, interval));
        if let Some(prior) = prior {
            if prior.answered {
                chosen = Some(Chosen {
                    index,
                    alternative,
                    route: prior.route.clone(),
                    proven_at: prior.at.clone(),
                    proven_now: false,
                });
                break;
            }
            skipped.push(Unreachable {
                product: name.to_owned(),
                route: prior.route.clone(),
                cause: format!(
                    "{} (proven {}; proven again once the interval passes or a caller reports --failed {name})",
                    prior.cause.as_deref().unwrap_or("did not answer"),
                    prior.at
                ),
            });
            continue;
        }
        let proven = prove(family, alternative, consumer, interval).await;
        let at = now();
        proofs.push((
            key,
            Proof {
                answered: proven.outcome.is_ok(),
                at: at.clone(),
                route: proven.route.clone(),
                cause: proven.outcome.as_ref().err().cloned(),
                reported_by: reported.then(|| consumer.to_owned()),
            },
        ));
        match proven.outcome {
            Ok(()) => {
                chosen = Some(Chosen {
                    index,
                    alternative,
                    route: proven.route,
                    proven_at: at,
                    proven_now: true,
                });
                break;
            }
            Err(cause) => skipped.push(Unreachable {
                product: name.to_owned(),
                route: proven.route,
                cause,
            }),
        }
    }
    let served = Served {
        provider: chosen
            .as_ref()
            .map(|chosen| chosen.alternative.provider.as_str().to_owned()),
        route: chosen.as_ref().map(|chosen| chosen.route.clone()),
        at: now(),
        fallback: chosen.as_ref().is_some_and(|chosen| chosen.index > 0),
        skipped: skipped.clone(),
    };
    let family_id = family.id.as_str().to_owned();
    let consumer_id = consumer.to_owned();
    record::update(|record| {
        let entry = record.families.entry(family_id).or_default();
        for (name, proof) in proofs {
            entry.proven.insert(name, proof);
        }
        entry.served.insert(consumer_id, served);
    })?;
    let order: Vec<&'static str> = ordered
        .iter()
        .map(|alternative| alternative.provider.as_str())
        .collect();
    let Some(chosen) = chosen else {
        return Err(none_answers(family, consumer, &order, &skipped));
    };
    Ok(Resolution {
        family,
        consumer: consumer.to_owned(),
        chosen,
        skipped,
        order,
        interval,
        interval_unread,
    })
}

/// `stado capabilities resolve FAMILY --consumer C [--failed P]... [--json]`.
pub(crate) async fn run(
    family: &str,
    consumer: &str,
    failed: &[String],
    as_json: bool,
) -> Result<(), CmdError> {
    let family = super::declared_family(family).map_err(|error| error.machine_readable(as_json))?;
    let resolution = choose(family, consumer, failed, &[])
        .await
        .map_err(|error| error.machine_readable(as_json))?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&resolution.answer())?);
        return Ok(());
    }
    let chosen = &resolution.chosen;
    let proof = if chosen.proven_now {
        "proven now"
    } else {
        "kept proof"
    };
    println!(
        "{} for {consumer}: {} at {} ({proof} of {}{})",
        family.id,
        chosen.alternative.provider,
        chosen.route,
        chosen.proven_at,
        if chosen.index > 0 { "; a fallback" } else { "" }
    );
    for item in &resolution.skipped {
        println!(
            "  {} at {} did not answer: {}",
            item.product, item.route, item.cause
        );
    }
    if let Some(cause) = &resolution.interval_unread {
        println!("  no kept proof stands, as its interval cannot be read: {cause}");
    }
    Ok(())
}
