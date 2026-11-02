//! `stado capabilities call inference --consumer C`: one model request,
//! answered by the alternative of the family that answers it.
//!
//! The caller writes the OpenAI chat-completions request it would send to a
//! model gateway on standard input and reads the answer on standard output.
//! It names no gateway, no key and no alternative: Stado resolves the family
//! in its selected order (`resolve::choose`) and asks the chosen alternative,
//! so an alternative added to the catalog or an order the operator selects
//! changes every caller's answer without a line of any caller changing.
//!
//! - **brama**: the request goes to the route the service directory hands the
//!   consumer, with the consumer's Brama bearer (the `token` of the item
//!   playing `<consumer>-model-router`) and, when an item plays
//!   `<consumer>-agent-auth`, its request signature.
//! - **attestation**: the statement recorded for this exact request answers
//!   it; with none, the request is kept and refused, naming the commands that
//!   read it and record the answer.
//!
//! An alternative that cannot answer this request is named with its cause and
//! the next one is tried, the same request; the refusal when none answers
//! names every one.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::record::Unreachable;
use super::resolve::{choose, Resolution};
use crate::capabilities::alternatives::{Family, Reach};
use crate::cli::CmdError;

/// What one alternative did with the request.
enum Outcome {
    /// It answered: the answer, as the caller reads it.
    Answered(Value),
    /// It could not answer this request; the next alternative is asked.
    Passed(String),
    /// It refused the request itself: no other alternative is asked, since
    /// the request, not the alternative, is what failed.
    Refused(CmdError),
}

/// The role whose item holds `consumer`'s Brama bearer, in field `token`.
fn bearer_role(consumer: &str) -> String {
    format!("{consumer}-model-router")
}

/// The role whose item holds `consumer`'s request-signing secret, in field
/// `agent_auth_secret`.
fn signing_role(consumer: &str) -> String {
    format!("{consumer}-agent-auth")
}

/// One field of the item playing `role`, from the selected credential store;
/// `Ok(None)` when no item plays the role.
async fn role_field(role: &str, field: &str) -> Result<Option<String>, String> {
    let store = crate::cli::secrets::store().map_err(|error| error.to_string())?;
    let item = match crate::cli::secrets::item_in_role(&store, role).await {
        Ok(item) => item,
        Err(error) if error.failure == Some(crate::primitives::failure::FailureCode::NotFound) => {
            return Ok(None)
        }
        Err(error) => return Err(error.to_string()),
    };
    crate::cli::secrets::field_value(&store, &item, field)
        .await
        .map(Some)
        .map_err(|error| error.to_string())
}

/// HMAC-SHA256 of `message` under `secret`, hex: the signature Brama checks.
fn signature(secret: &[u8], message: &[u8]) -> String {
    const BLOCK: usize = 64;
    let mut key = if secret.len() > BLOCK {
        Sha256::digest(secret).to_vec()
    } else {
        secret.to_vec()
    };
    key.resize(BLOCK, 0);
    let pad = |byte: u8| key.iter().map(|k| k ^ byte).collect::<Vec<u8>>();
    let inner = Sha256::new()
        .chain_update(pad(0x36))
        .chain_update(message)
        .finalize();
    let outer = Sha256::new()
        .chain_update(pad(0x5c))
        .chain_update(inner)
        .finalize();
    hex::encode(outer)
}

/// Ask Brama at `route` for `consumer`.
async fn brama(route: &str, consumer: &str, body: &str) -> Outcome {
    let role = bearer_role(consumer);
    let bearer = match role_field(&role, "token").await {
        Ok(Some(bearer)) => bearer,
        Ok(None) => {
            return Outcome::Passed(format!(
                "no item plays role {role}, which holds {consumer}'s Brama bearer in field token; store it with `stado credentials item put --host <vault owner> --role {role}`"
            ))
        }
        Err(cause) => {
            return Outcome::Passed(format!("{consumer}'s Brama bearer (role {role}): {cause}"))
        }
    };
    let signer = signing_role(consumer);
    let secret = match role_field(&signer, "agent_auth_secret").await {
        Ok(secret) => secret,
        Err(cause) => {
            return Outcome::Passed(format!(
                "{consumer}'s request-signing secret (role {signer}): {cause}"
            ))
        }
    };
    let client = match reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(error) => return Outcome::Passed(crate::cli::http_failure(&error)),
    };
    let url = format!("{}/v1/chat/completions", route.trim_end_matches('/'));
    let mut request = client
        .post(&url)
        .bearer_auth(bearer.trim())
        .header("content-type", "application/json");
    if let Some(secret) = secret {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or_default()
            .to_string();
        let digest = hex::encode(Sha256::digest(body.as_bytes()));
        let signed = signature(
            secret.as_bytes(),
            format!("{consumer}:{timestamp}:{digest}").as_bytes(),
        );
        request = request
            .header("x-agent-id", consumer)
            .header("x-agent-timestamp", timestamp)
            .header("x-agent-signature", signed);
    }
    let response = match crate::wait::request(request.body(body.to_owned())).await {
        Ok(response) => response,
        Err(error) => {
            return Outcome::Passed(format!("{url}: {}", crate::cli::http_failure(&error)))
        }
    };
    let status = response.status();
    let text = match response.text().await {
        Ok(text) => text,
        Err(error) => {
            return Outcome::Passed(format!(
                "{url} answered {status} with a body that cannot be read: {}",
                crate::cli::http_failure(&error)
            ))
        }
    };
    let answer: Option<Value> = serde_json::from_str(&text).ok();
    if status.is_success() {
        return match answer {
            Some(answer) => Outcome::Answered(answer),
            None => Outcome::Passed(format!(
                "{url} answered {status} with a body that is not JSON: {}",
                text.trim()
            )),
        };
    }
    let said = answer
        .as_ref()
        .and_then(|answer| answer["error"]["message"].as_str().map(str::to_owned))
        .unwrap_or_else(|| text.trim().to_owned());
    // A request Brama judged malformed fails at every alternative alike; any
    // other status is this alternative unable to answer now.
    if status == reqwest::StatusCode::BAD_REQUEST
        || status == reqwest::StatusCode::UNPROCESSABLE_ENTITY
    {
        return Outcome::Refused(CmdError::refused(format!(
            "Brama at {route} refused the request itself ({status}): {said}"
        )));
    }
    Outcome::Passed(format!("{url} answered {status}: {said}"))
}

/// Answer the request from the statement recorded for it, or keep it.
fn attested(family: &Family, consumer: &str, request: &Value) -> Outcome {
    match super::attestation::answer_or_keep(family, consumer, request) {
        Ok((_, Some(answer))) => Outcome::Answered(answer),
        Ok((digest, None)) => Outcome::Passed(format!(
            "no statement answers request {digest} yet; read it with `stado capabilities attestation show {} --consumer {consumer} {digest}` and answer it with `stado capabilities attestation record {} --consumer {consumer} {digest} --checked <what was checked>`, the answer on standard input",
            family.id, family.id
        )),
        Err(cause) => Outcome::Passed(cause),
    }
}

async fn ask(resolution: &Resolution, request: &Value, body: &str) -> Outcome {
    let consumer = resolution.consumer.as_str();
    match resolution.chosen.alternative.reach {
        Reach::Directory { .. } => brama(&resolution.chosen.route, consumer, body).await,
        Reach::Attestation => attested(resolution.family, consumer, request),
        Reach::Program { name } => Outcome::Passed(format!(
            "the program {name} answers no {} request",
            resolution.family.id
        )),
        Reach::OwnerFile => Outcome::Passed(format!(
            "the owner file answers no {} request",
            resolution.family.id
        )),
    }
}

/// `stado capabilities call FAMILY --consumer C [--json]`, the request on
/// standard input.
pub(crate) async fn run(
    family: &str,
    consumer: &str,
    input: &str,
    as_json: bool,
) -> Result<(), CmdError> {
    let refuse = |error: CmdError| error.machine_readable(as_json);
    let family = super::declared_family(family).map_err(refuse)?;
    if family.id.as_str() != "inference" {
        return Err(refuse(CmdError::usage(format!(
            "{} is resolved, not called: a product asks `stado capabilities resolve {} --consumer {consumer}` and reaches what it hands out",
            family.id, family.id
        ))));
    }
    let request: Value = serde_json::from_str(input).map_err(|error| {
        refuse(CmdError::usage(format!(
            "standard input is not one JSON chat-completions request: {error}"
        )))
    })?;
    if !request["messages"].is_array() {
        return Err(refuse(CmdError::usage(
            "the request names no messages: standard input is an OpenAI chat-completions request",
        )));
    }
    let body = request.to_string();
    let mut tried: Vec<Unreachable> = Vec::new();
    loop {
        // Once every alternative was tried for this request, choose refuses
        // naming each one with the cause it gave.
        let resolution = choose(family, consumer, &[], &tried)
            .await
            .map_err(refuse)?;
        let provider = resolution.chosen.alternative.provider.as_str();
        match ask(&resolution, &request, &body).await {
            Outcome::Answered(answer) => {
                let printed = if as_json {
                    json!({"resolution": resolution.answer(), "answer": answer})
                } else {
                    answer
                };
                println!("{}", serde_json::to_string(&printed)?);
                return Ok(());
            }
            Outcome::Passed(cause) => tried.push(Unreachable {
                product: provider.to_owned(),
                route: resolution.chosen.route.clone(),
                cause,
            }),
            Outcome::Refused(error) => return Err(refuse(error)),
        }
    }
}
