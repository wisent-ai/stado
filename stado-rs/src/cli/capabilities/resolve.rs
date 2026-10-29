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
use crate::capabilities::alternatives::{self, Alternative, Family, Reach};
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
        let said = String::from_utf8_lossy(&output.stderr);
        let cause = said
            .lines()
            .rev()
            .find(|line| line.starts_with("Error: "))
            .or_else(|| said.lines().rev().find(|line| !line.trim().is_empty()))
            .unwrap_or("it exited without saying why")
            .trim_start_matches("Error: ")
            .to_owned();
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

/// Prove the endpoint the operator declares answers HTTP at all: any status
/// is an answer, as a gateway refusing an unauthenticated caller has
/// answered.
async fn endpoint(url_field: &crate::capabilities::ConfigField, bound: Option<Duration>) -> Proven {
    let Some(url) = declared(url_field) else {
        return Proven {
            route: url_field.path.to_owned(),
            outcome: Err(format!(
                "no endpoint is declared: set {} (or {})",
                url_field.path, url_field.env
            )),
        };
    };
    let client = match reqwest::Client::builder().no_proxy().build() {
        Ok(client) => client,
        Err(error) => {
            return Proven {
                route: url,
                outcome: Err(crate::cli::http_failure(&error)),
            }
        }
    };
    let models = format!("{}/v1/models", url.trim_end_matches('/'));
    let mut request = client.get(&models);
    if let Some(bound) = bound {
        request = request.timeout(bound);
    }
    let outcome = crate::wait::request(request)
        .await
        .map(|_answered| ())
        .map_err(|error| crate::cli::http_failure(&error));
    Proven {
        route: url,
        outcome,
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

async fn prove(alternative: &Alternative, consumer: &str, bound: Option<Duration>) -> Proven {
    match alternative.reach {
        Reach::Directory { service } => directory(service, consumer, bound).await,
        Reach::Endpoint { url, .. } => endpoint(url, bound).await,
        Reach::Program { name } => program(name),
        Reach::OwnerFile => owner_file(),
    }
}

fn kind(reach: &Reach) -> &'static str {
    match reach {
        Reach::Directory { .. } => "directory",
        Reach::Endpoint { .. } => "endpoint",
        Reach::Program { .. } => "program",
        Reach::OwnerFile => "owner-file",
    }
}

/// A declared text setting, trimmed; `None` when unset or blank.
fn declared(field: &crate::capabilities::ConfigField) -> Option<String> {
    crate::config_file::field_value(field)
        .and_then(|value| value.as_str().map(str::trim).map(str::to_owned))
        .filter(|text| !text.is_empty())
}

/// What the caller of an endpoint alternative asks it for and presents to
/// it, as declared: the model, and the role of the credential-store item
/// holding its key, which the caller reads itself (`stado credentials get
/// --role ROLE`); never the secret. Both are `None` for every other kind.
fn endpoint_terms(reach: &Reach) -> (Option<String>, Option<String>) {
    match reach {
        Reach::Endpoint {
            model, key_role, ..
        } => (declared(model), declared(key_role)),
        _ => (None, None),
    }
}

/// The alternative that answers, where, and whether it was proven now or
/// its kept proof stands.
struct Chosen<'a> {
    index: usize,
    alternative: &'a Alternative,
    route: String,
    proven_at: String,
    proven_now: bool,
}

/// `stado capabilities resolve FAMILY --consumer C [--failed P]... [--json]`.
pub(crate) async fn run(
    family: &str,
    consumer: &str,
    failed: &[String],
    as_json: bool,
) -> Result<(), CmdError> {
    let family = alternatives::family(family).ok_or_else(|| {
        let known: Vec<&str> = alternatives::FAMILIES
            .iter()
            .map(|family| family.id.as_str())
            .collect();
        CmdError::usage(format!(
            "{family:?} is no family a product resolves through Stado; the families are [{}]",
            known.join(", ")
        ))
        .machine_readable(as_json)
    })?;
    if consumer.trim().is_empty() {
        return Err(
            CmdError::usage("--consumer names the product asking").machine_readable(as_json)
        );
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
            ))
            .machine_readable(as_json));
        }
    }
    let ordered = order(family).map_err(|error| error.machine_readable(as_json))?;
    let (interval, interval_unread) = interval();
    let kept = record::read()
        .map_err(|error| error.machine_readable(as_json))?
        .families
        .remove(family.id.as_str())
        .unwrap_or_default();

    let mut proofs: Vec<(String, Proof)> = Vec::new();
    let mut skipped: Vec<Unreachable> = Vec::new();
    let mut chosen: Option<Chosen> = None;
    for (index, alternative) in ordered.iter().enumerate() {
        let name = alternative.provider.as_str();
        // A directory route is admitted and placed per consumer, so its
        // proof speaks for that consumer alone; an endpoint, a program and
        // the owner file answer every consumer alike.
        let key = match alternative.reach {
            Reach::Directory { .. } => format!("{name} for {consumer}"),
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
        let proven = prove(alternative, consumer, interval).await;
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
    })
    .map_err(|error| error.machine_readable(as_json))?;

    let order_names: Vec<&str> = ordered
        .iter()
        .map(|alternative| alternative.provider.as_str())
        .collect();
    let unreachable: Vec<Value> = skipped
        .iter()
        .map(|item| json!({"product": item.product, "route": item.route, "cause": item.cause}))
        .collect();
    let Some(Chosen {
        index,
        alternative,
        route,
        proven_at,
        proven_now,
    }) = chosen
    else {
        let sentence = skipped
            .iter()
            .map(|item| format!("{} at {}: {}", item.product, item.route, item.cause))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(CmdError::unreachable(format!(
            "no {} alternative answers {consumer}: {sentence}",
            family.id
        ))
        .detailing(json!({
            "family": family.id.as_str(),
            "consumer": consumer,
            "order": order_names,
            "unreachable": unreachable,
        }))
        .machine_readable(as_json));
    };
    let (model, key_role) = endpoint_terms(&alternative.reach);
    let answer = json!({
        "family": family.id.as_str(),
        "consumer": consumer,
        "provider": alternative.provider.as_str(),
        "kind": kind(&alternative.reach),
        "route": route,
        "model": model,
        "key_role": key_role,
        "proven_at": proven_at,
        "proven_now": proven_now,
        "fallback": index > 0,
        "order": order_names,
        "unreachable": unreachable,
        "interval_seconds": interval.map(|interval| interval.as_secs()),
        "interval_unread": interval_unread,
    });
    if as_json {
        println!("{}", serde_json::to_string_pretty(&answer)?);
    } else {
        let proof = if proven_now {
            "proven now"
        } else {
            "kept proof"
        };
        println!(
            "{} for {consumer}: {} at {route} ({proof} of {proven_at}{})",
            family.id,
            alternative.provider,
            if index > 0 { "; a fallback" } else { "" }
        );
        for item in &skipped {
            println!(
                "  {} at {} did not answer: {}",
                item.product, item.route, item.cause
            );
        }
    }
    Ok(())
}
