//! `stado inference gateway <deployment> --listen ADDR:PORT`: the one door an
//! outside caller reaches a deployment through.
//!
//! Every request must present `Authorization: Bearer <key>`; the key's
//! SHA-256 is compared with each declared caller of the deployment
//! ([`crate::inference::callers`]), in constant time. A revoked caller, an
//! unknown key, a caller past its requests per minute or its tokens for the
//! UTC day, and a deployment whose model declares no license are refused
//! before the engine is asked, each with the JSON body `{"error": {...}}`
//! naming the limit or the cause. An admitted request is sent to the engine
//! with the deployment's own engine key, and the answer's `usage` (prompt
//! and completion tokens) is added to the caller's hour; an answer that
//! states no usage is counted in `usage_unread`, never as zero tokens.
//! Streaming answers are refused: their usage arrives only in a final chunk
//! the gateway would have to parse while relaying, and a request whose
//! tokens cannot be counted cannot be billed.
//!
//! Usage is held in this process and written to the fleet store at most once
//! per `--flush-seconds` and on the hour's change, so a request is never
//! held for a store write.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

use crate::cli::CmdError;
use crate::dashboard::{read_request, RequestLimits};
use crate::inference::callers::{self, Caller, HourUsage};
use crate::inference::schema::Deployment;
use crate::queue::JobStorage;

/// What the gateway holds between requests.
struct State {
    deployment: Deployment,
    engine_key: String,
    callers: Vec<Caller>,
    /// Per caller: request instants in the last minute.
    recent: BTreeMap<String, Vec<Instant>>,
    /// Per caller: the UTC day and the tokens spent in it.
    day_tokens: BTreeMap<String, (String, u64)>,
    /// Per caller: the hour being counted.
    hours: BTreeMap<String, HourUsage>,
    last_flush: Instant,
}

fn refusal(status: u16, kind: &str, message: &str) -> Vec<u8> {
    let body = json!({"error": {"type": kind, "message": message}}).to_string();
    let reason = reqwest::StatusCode::from_u16(status)
        .ok()
        .and_then(|code| code.canonical_reason())
        .unwrap_or("");
    let mut bytes = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    bytes.extend_from_slice(body.as_bytes());
    bytes
}

fn answer(status: u16, body: &[u8]) -> Vec<u8> {
    let reason = reqwest::StatusCode::from_u16(status)
        .ok()
        .and_then(|code| code.canonical_reason())
        .unwrap_or("");
    let mut bytes = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

/// Write every held hour to the store.
async fn flush(store: &JobStorage, state: &mut State) {
    for hour in state.hours.values() {
        if let Err(error) = callers::save_usage(store, hour).await {
            eprintln!(
                "stado inference gateway: usage of {} for {} is not kept yet: {error}",
                hour.caller, hour.hour
            );
        }
    }
    state.last_flush = Instant::now();
}

/// The caller presenting `bearer`, if any declared caller's key hashes to it.
fn presented<'a>(callers: &'a [Caller], bearer: &str) -> Option<&'a Caller> {
    let hash = callers::key_hash(bearer);
    callers.iter().find(|caller| {
        crate::dashboard::constant_time_eq(caller.key_sha256.as_bytes(), hash.as_bytes())
    })
}

async fn serve_one(
    mut stream: TcpStream,
    limits: RequestLimits,
    store: JobStorage,
    state: Arc<Mutex<State>>,
    client: reqwest::Client,
    flush_every: Duration,
) -> std::io::Result<()> {
    let mut carry = Vec::new();
    let Some(request) = read_request(&mut stream, &mut carry, limits).await? else {
        return Ok(());
    };
    let (route, _) = request.target();
    if request.method != "POST" || !route.starts_with("/v1/") {
        return stream
            .write_all(&refusal(
                404,
                "not_found",
                "this gateway answers POST /v1/... OpenAI-compatible requests only",
            ))
            .await;
    }
    let Some(bearer) = request
        .header("authorization")
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        return stream
            .write_all(&refusal(
                401,
                "unauthorized",
                "send Authorization: Bearer <your key>",
            ))
            .await;
    };
    let body: Value = match serde_json::from_slice(&request.body) {
        Ok(body) => body,
        Err(error) => {
            return stream
                .write_all(&refusal(
                    400,
                    "invalid_request",
                    &format!("the body is not JSON: {error}"),
                ))
                .await
        }
    };
    if body["stream"].as_bool() == Some(true) {
        return stream
            .write_all(&refusal(
                400,
                "streaming_not_metered",
                "streaming answers are not served: their tokens cannot be counted for billing; send \"stream\": false",
            ))
            .await;
    }
    let now = chrono::Utc::now();
    let day = now.date_naive().to_string();
    let hour = callers::hour_of(now);
    // Admission: who is calling, and whether a limit refuses them.
    let (caller_name, engine_url, engine_key) = {
        let mut held = state.lock().await;
        let Some(caller) = presented(&held.callers, bearer).cloned() else {
            return stream
                .write_all(&refusal(
                    401,
                    "unauthorized",
                    "the key is not one this deployment issued",
                ))
                .await;
        };
        if let Some(revoked) = &caller.revoked_at {
            return stream
                .write_all(&refusal(
                    401,
                    "revoked",
                    &format!("this key was revoked at {revoked}"),
                ))
                .await;
        }
        let window = held.recent.entry(caller.caller.clone()).or_default();
        window.retain(|at| at.elapsed() < Duration::from_secs(60));
        let over_rate = window.len() >= caller.limits.requests_per_minute as usize;
        if !over_rate {
            window.push(Instant::now());
        }
        let spent = match held.day_tokens.get(&caller.caller) {
            Some((counted_day, tokens)) if *counted_day == day => *tokens,
            _ => 0,
        };
        let over_day = spent >= caller.limits.tokens_per_day;
        if over_rate || over_day {
            // A refusal is counted in the hour it happens in: an hour held
            // from before is written first, as an answered request's is.
            if held
                .hours
                .get(&caller.caller)
                .is_some_and(|usage| usage.hour != hour)
            {
                flush(&store, &mut held).await;
                held.hours.remove(&caller.caller);
            }
            let usage = held
                .hours
                .entry(caller.caller.clone())
                .or_insert_with(|| HourUsage {
                    caller: caller.caller.clone(),
                    deployment: caller.deployment.clone(),
                    hour: hour.clone(),
                    ..HourUsage::default()
                });
            usage.refused += 1;
            let message = if over_rate {
                format!(
                    "limit reached: {} requests per minute",
                    caller.limits.requests_per_minute
                )
            } else {
                format!(
                    "limit reached: {} of {} tokens for {day} (UTC) spent",
                    spent, caller.limits.tokens_per_day
                )
            };
            return stream
                .write_all(&refusal(429, "rate_limited", &message))
                .await;
        }
        let deployment = &held.deployment;
        (
            caller.caller,
            format!(
                "http://{}:{}{}",
                deployment.endpoint.host, deployment.endpoint.port, request.path
            ),
            held.engine_key.clone(),
        )
    };
    let started = Instant::now();
    let sent = crate::wait::request(
        client
            .post(&engine_url)
            .bearer_auth(&engine_key)
            .json(&body),
    )
    .await;
    let (status, bytes) = match sent {
        Ok(response) => {
            let status = response.status().as_u16();
            match response.bytes().await {
                Ok(bytes) => (status, bytes.to_vec()),
                Err(error) => {
                    return stream
                        .write_all(&refusal(
                            502,
                            "engine_unreadable",
                            &format!("the engine's answer could not be read: {error}"),
                        ))
                        .await
                }
            }
        }
        Err(error) => {
            return stream
                .write_all(&refusal(
                    502,
                    "engine_unreachable",
                    &format!("the engine did not answer: {error}"),
                ))
                .await
        }
    };
    let seconds = started.elapsed().as_secs_f64();
    {
        let mut held = state.lock().await;
        let stated: Option<(u64, u64)> =
            serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|answer| {
                    Some((
                        answer["usage"]["prompt_tokens"].as_u64()?,
                        answer["usage"]["completion_tokens"].as_u64()?,
                    ))
                });
        let changed_hour = held
            .hours
            .get(&caller_name)
            .is_some_and(|usage| usage.hour != hour);
        if changed_hour {
            flush(&store, &mut held).await;
            held.hours.remove(&caller_name);
        }
        let deployment = held.deployment.name.clone();
        let usage = held
            .hours
            .entry(caller_name.clone())
            .or_insert_with(|| HourUsage {
                caller: caller_name.clone(),
                deployment,
                hour: hour.clone(),
                ..HourUsage::default()
            });
        usage.requests += 1;
        usage.seconds += seconds;
        match stated {
            Some((prompt, generated)) => {
                usage.prompt_tokens += prompt;
                usage.generated_tokens += generated;
                let entry = held
                    .day_tokens
                    .entry(caller_name.clone())
                    .or_insert_with(|| (day.clone(), 0));
                if entry.0 != day {
                    *entry = (day.clone(), 0);
                }
                entry.1 += prompt + generated;
            }
            None if status < 400 => usage.usage_unread += 1,
            None => {}
        }
        if held.last_flush.elapsed() >= flush_every {
            flush(&store, &mut held).await;
        }
    }
    stream.write_all(&answer(status, &bytes)).await
}

/// The day's tokens each caller already spent, from the kept usage, so a
/// restarted gateway does not hand a caller its day again.
async fn spent_today(
    store: &JobStorage,
    deployment: &str,
) -> Result<(BTreeMap<String, (String, u64)>, BTreeMap<String, HourUsage>), CmdError> {
    let now = chrono::Utc::now();
    let day = now.date_naive().to_string();
    let this_hour = callers::hour_of(now);
    let mut spent: BTreeMap<String, (String, u64)> = BTreeMap::new();
    let mut hours = BTreeMap::new();
    for hour in callers::usage(store, deployment, None, &format!("{day}T00:00:00Z")).await? {
        spent
            .entry(hour.caller.clone())
            .or_insert_with(|| (day.clone(), 0))
            .1 += hour.tokens();
        // The hour in progress is continued, not restarted at zero.
        if hour.hour == this_hour {
            hours.insert(hour.caller.clone(), hour);
        }
    }
    Ok((spent, hours))
}

pub async fn run(
    name: &str,
    listen: &str,
    flush_seconds: u64,
    refresh_seconds: u64,
) -> Result<(), CmdError> {
    if flush_seconds == 0 || refresh_seconds == 0 {
        return Err(CmdError::usage(
            "--flush-seconds and --refresh-seconds must be above zero",
        ));
    }
    let document = crate::cli::registry::fetch_document().await?;
    let registry = crate::inference::schema::parse(&document).map_err(CmdError::declaration)?;
    let deployment = registry
        .deployments
        .into_iter()
        .find(|deployment| deployment.name == name)
        .ok_or_else(|| CmdError::refused(format!("unknown inference deployment '{name}'")))?;
    // A model whose license chain is unread or declares none is not offered
    // to anyone outside the fleet.
    match &deployment.model.license {
        None => {
            return Err(CmdError::refused(format!(
                "{name}'s model license was never read, so it is not offered to outside callers; \
                 plan the deployment again to read it"
            )))
        }
        Some(license) if !license.undeclared().is_empty() => {
            return Err(CmdError::refused(format!(
                "{name}'s model chain declares no license for {}, so it is not offered to \
                 outside callers ({})",
                license.undeclared().join(", "),
                license.summary()
            )))
        }
        Some(_) => {}
    }
    let engine_key = super::credential::read().await?;
    let store = JobStorage::new().await?;
    let declared = callers::list(&store, name).await?;
    let (day_tokens, hours) = spent_today(&store, name).await?;
    let state = Arc::new(Mutex::new(State {
        deployment,
        engine_key,
        callers: declared,
        recent: BTreeMap::new(),
        day_tokens,
        hours,
        last_flush: Instant::now(),
    }));
    let limits = RequestLimits::read().map_err(|error| CmdError::declaration(error.to_string()))?;
    let listener = TcpListener::bind(listen).await.map_err(|error| {
        CmdError::declaration(format!("{listen} cannot be listened on: {error}"))
    })?;
    let client = reqwest::Client::new();
    let flush_every = Duration::from_secs(flush_seconds);
    println!("stado inference gateway: {name} served on {listen}");
    // Callers issued or revoked after the start are read again on this
    // cadence, and held usage is written even with no traffic.
    let refresher = {
        let state = Arc::clone(&state);
        let store = store.clone();
        let name = name.to_string();
        tokio::spawn(async move {
            let mut cadence = tokio::time::interval(Duration::from_secs(refresh_seconds));
            loop {
                cadence.tick().await;
                match callers::list(&store, &name).await {
                    Ok(fresh) => state.lock().await.callers = fresh,
                    Err(error) => eprintln!(
                        "stado inference gateway: the callers of {name} were not read again: {error}"
                    ),
                }
                let mut held = state.lock().await;
                flush(&store, &mut held).await;
            }
        })
    };
    loop {
        let (stream, _) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(error) => {
                refresher.abort();
                return Err(CmdError::declaration(format!(
                    "{listen} stopped accepting: {error}"
                )));
            }
        };
        let state = Arc::clone(&state);
        let store = store.clone();
        let client = client.clone();
        tokio::spawn(async move {
            if let Err(error) = serve_one(stream, limits, store, state, client, flush_every).await {
                eprintln!("stado inference gateway: a connection ended: {error}");
            }
        });
    }
}
