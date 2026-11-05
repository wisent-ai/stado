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
//! A deployment with a pay-per-call offer ([`crate::inference::pay_per_call`])
//! also sells one call to a caller with no key: a request without a key is
//! answered `402 Payment Required` with a quote, `POST /v1/pay` with
//! `{"quote", "method", "reference"}` asks the method's endpoint whether the
//! reference settled the quote and answers a key good for one call, and that
//! call is held to the quote's generated-token bound and settled — paid, cost,
//! refund due — when it returns.
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
use crate::inference::pay_per_call::{self, Offer};
use crate::inference::schema::Deployment;
use crate::queue::JobStorage;

/// What the gateway holds between requests.
struct State {
    deployment: Deployment,
    engine_key: String,
    callers: Vec<Caller>,
    /// The deployment's pay-per-call offer, when it sells calls without a
    /// key.
    offer: Option<Offer>,
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
    if route == "/v1/pay" {
        let answered = pay(&request.body, &store, &state, &client).await;
        return stream.write_all(&answered).await;
    }
    let Some(bearer) = request
        .header("authorization")
        .and_then(|value| value.strip_prefix("Bearer "))
    else {
        let answered = unpaid(&store, &state).await;
        return stream.write_all(&answered).await;
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
    if bearer.starts_with(pay_per_call::KEY_PREFIX) {
        let answered = paid_call(bearer, body, &request.path, &store, &state, &client).await;
        return stream.write_all(&answered).await;
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

/// A request with no key: a quote when the deployment sells calls, else the
/// refusal it always had.
async fn unpaid(store: &JobStorage, state: &Arc<Mutex<State>>) -> Vec<u8> {
    let offer = state.lock().await.offer.clone();
    let Some(offer) = offer else {
        return refusal(
            401,
            "unauthorized",
            "send Authorization: Bearer <your key>; this deployment sells no call without one",
        );
    };
    match pay_per_call::quote(store, &offer).await {
        Ok(quote) => {
            let body = json!({
                "error": {
                    "type": "payment_required",
                    "message": format!(
                        "one call costs at most {:.6} USD; pay quote {} by one of the methods, \
                         then POST /v1/pay with {{\"quote\", \"method\", \"reference\"}} for a \
                         key good for one call",
                        quote.usd, quote.id
                    ),
                },
                "quote": quote,
            });
            answer(402, body.to_string().as_bytes())
        }
        Err(error) => refusal(
            503,
            "quote_unkept",
            &format!("a quote could not be kept, so none is stated: {error}"),
        ),
    }
}

/// `POST /v1/pay`: verify a payment with the method's endpoint and answer a
/// one-time key.
async fn pay(
    body: &[u8],
    store: &JobStorage,
    state: &Arc<Mutex<State>>,
    client: &reqwest::Client,
) -> Vec<u8> {
    let (offer, deployment) = {
        let held = state.lock().await;
        (held.offer.clone(), held.deployment.name.clone())
    };
    let Some(offer) = offer else {
        return refusal(
            404,
            "not_sold",
            "this deployment sells no call without a key",
        );
    };
    let posted: Value = match serde_json::from_slice(body) {
        Ok(posted) => posted,
        Err(error) => {
            return refusal(
                400,
                "invalid_request",
                &format!("the body is not JSON: {error}"),
            )
        }
    };
    let (Some(quote_id), Some(method_name), Some(reference)) = (
        posted["quote"].as_str(),
        posted["method"].as_str(),
        posted["reference"]
            .as_str()
            .filter(|reference| !reference.is_empty()),
    ) else {
        return refusal(
            400,
            "invalid_request",
            "send {\"quote\": <id>, \"method\": <name>, \"reference\": <the payment's reference>}",
        );
    };
    let quote = match pay_per_call::load_quote(store, &deployment, quote_id).await {
        Ok(Some(quote)) => quote,
        Ok(None) => {
            return refusal(
                404,
                "unknown_quote",
                &format!("no quote {quote_id} was stated"),
            )
        }
        Err(error) => {
            return refusal(
                503,
                "quote_unread",
                &format!("quote {quote_id} could not be read: {error}"),
            )
        }
    };
    if chrono::DateTime::parse_from_rfc3339(&quote.expires_at)
        .is_ok_and(|expires| expires < chrono::Utc::now())
    {
        return refusal(
            410,
            "quote_expired",
            &format!(
                "quote {} expired at {}; ask for a new one",
                quote.id, quote.expires_at
            ),
        );
    }
    let Some(method) = offer
        .methods
        .iter()
        .find(|method| method.name == method_name)
    else {
        return refusal(
            400,
            "unknown_method",
            &format!(
                "{method_name} is not a payment method of this offer; the methods are {}",
                quote.methods.join(", ")
            ),
        );
    };
    let mut asked = client.post(&method.verify_url).json(&json!({
        "quote": quote.id,
        "deployment": deployment,
        "usd": quote.usd,
        "reference": reference,
    }));
    if let Some(item) = &method.credential_item {
        let bearer = match crate::skarbiec::Client::configured() {
            Ok(vault) => vault
                .read_field(item, "token")
                .await
                .map_err(|error| error.to_string()),
            Err(error) => Err(error.to_string()),
        };
        let token = match &bearer {
            Ok(value) => value
                .as_str()
                .ok_or_else(|| "it has no string field token".to_string()),
            Err(error) => Err(format!("it could not be read: {error}")),
        };
        match token {
            Ok(token) => asked = asked.bearer_auth(token),
            Err(cause) => {
                let message = format!("payment method {method_name}'s vault item {item}: {cause}");
                return refusal(503, "method_credential_unread", &message);
            }
        }
    }
    let verdict: Result<Value, String> = match crate::wait::request(asked).await {
        Ok(response) if response.status().is_success() => response
            .json()
            .await
            .map_err(|error| format!("answered something that is not JSON: {error}")),
        Ok(response) => Err(format!(
            "refused the verification with HTTP {}",
            response.status()
        )),
        Err(error) => Err(format!("did not answer: {error}")),
    };
    let verdict = match verdict {
        Ok(verdict) => verdict,
        Err(cause) => {
            let message = format!("payment method {method_name} {cause}");
            return refusal(502, "payment_unverified", &message);
        }
    };
    let (Some(settled), Some(usd_paid)) = (verdict["settled"].as_bool(), verdict["usd"].as_f64())
    else {
        return refusal(
            502,
            "payment_unverified",
            &format!("payment method {method_name} did not state settled and usd: {verdict}"),
        );
    };
    if !settled || usd_paid < quote.usd {
        return refusal(
            402,
            "payment_insufficient",
            &format!(
                "payment method {method_name} states {} with {usd_paid} USD paid; quote {} needs {} USD",
                if settled { "a settled payment" } else { "no settled payment" },
                quote.id,
                quote.usd
            ),
        );
    }
    match pay_per_call::claim_reference(store, &deployment, method_name, reference, &quote.id).await
    {
        Ok(true) => {}
        Ok(false) => {
            return refusal(
                409,
                "reference_used",
                "this payment reference was already exchanged for a key",
            )
        }
        Err(error) => {
            return refusal(
                503,
                "payment_unkept",
                &format!("the payment could not be kept: {error}"),
            )
        }
    }
    match pay_per_call::mint(store, &quote, method_name, reference, usd_paid).await {
        Ok(key) => answer(
            200,
            json!({"key": key, "quote": quote.id, "usd_paid": usd_paid, "calls": 1})
                .to_string()
                .as_bytes(),
        ),
        Err(error) => refusal(
            503,
            "payment_unkept",
            &format!(
                "the payment was verified and its reference claimed, but its key could not be \
                 kept: {error}; the payment is owed back"
            ),
        ),
    }
}

/// One call with a one-time key: spent before the engine is asked, held to
/// the quote's generated-token bound, settled when it returns.
async fn paid_call(
    key: &str,
    mut body: Value,
    path: &str,
    store: &JobStorage,
    state: &Arc<Mutex<State>>,
    client: &reqwest::Client,
) -> Vec<u8> {
    let (deployment, engine_url, engine_key) = {
        let held = state.lock().await;
        (
            held.deployment.name.clone(),
            format!(
                "http://{}:{}{}",
                held.deployment.endpoint.host, held.deployment.endpoint.port, path
            ),
            held.engine_key.clone(),
        )
    };
    let payment = match pay_per_call::payment_for(store, &deployment, key).await {
        Ok(Some(payment)) => payment,
        Ok(None) => {
            return refusal(
                401,
                "unauthorized",
                "the key is not one this deployment sold",
            )
        }
        Err(error) => {
            return refusal(
                503,
                "payment_unread",
                &format!("the key's payment could not be read: {error}"),
            )
        }
    };
    let quote = match pay_per_call::load_quote(store, &deployment, &payment.quote).await {
        Ok(Some(quote)) => quote,
        Ok(None) => {
            return refusal(
                503,
                "quote_unread",
                &format!("quote {} of this key is not kept", payment.quote),
            )
        }
        Err(error) => {
            return refusal(
                503,
                "quote_unread",
                &format!("quote {} could not be read: {error}", payment.quote),
            )
        }
    };
    match pay_per_call::spend(store, &payment).await {
        Ok(true) => {}
        Ok(false) => return refusal(401, "spent", "this key paid for one call, which was made"),
        Err(error) => {
            return refusal(
                503,
                "spend_unkept",
                &format!("the key could not be marked spent: {error}"),
            )
        }
    }
    // The quote's bound holds even when the offer changed or was withdrawn
    // after it was paid.
    let asked = body["max_tokens"].as_u64();
    if asked.is_none_or(|asked| asked > quote.max_generated_tokens) {
        body["max_tokens"] = json!(quote.max_generated_tokens);
    }
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
                Ok(bytes) => (Some(status), Ok(bytes.to_vec())),
                Err(error) => (
                    Some(status),
                    Err(format!("the engine's answer could not be read: {error}")),
                ),
            }
        }
        Err(error) => (None, Err(format!("the engine did not answer: {error}"))),
    };
    let stated = bytes.as_ref().ok().and_then(|bytes| {
        let answer: Value = serde_json::from_slice(bytes).ok()?;
        Some((
            answer["usage"]["prompt_tokens"].as_u64()?,
            answer["usage"]["completion_tokens"].as_u64()?,
        ))
    });
    let served_status = match (&bytes, status) {
        (Ok(_), status) => status,
        (Err(_), _) => None,
    };
    let settlement = pay_per_call::Settlement::of(
        &payment,
        &quote.price,
        served_status,
        stated,
        started.elapsed().as_secs_f64(),
    );
    if let Err(error) = pay_per_call::save_settlement(store, &settlement).await {
        eprintln!(
            "stado inference gateway: the settlement of quote {} is not kept: {error}",
            settlement.quote
        );
    }
    match bytes {
        Ok(bytes) => answer(status.unwrap_or(502), &bytes),
        Err(cause) => refusal(
            502,
            "engine_failed",
            &format!("{cause}; the call is settled as failed and its whole payment is owed back"),
        ),
    }
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
    let offer = pay_per_call::offer(&store, name).await?;
    let (day_tokens, hours) = spent_today(&store, name).await?;
    let state = Arc::new(Mutex::new(State {
        deployment,
        engine_key,
        callers: declared,
        offer,
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
                match pay_per_call::offer(&store, &name).await {
                    Ok(fresh) => state.lock().await.offer = fresh,
                    Err(error) => eprintln!(
                        "stado inference gateway: the pay-per-call offer of {name} was not read \
                         again: {error}"
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
