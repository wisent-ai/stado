//! Wisent's contribution ledger as `stado contributions` reaches it: where it
//! is, the bearer Stado reports with, which events it already holds, and
//! recording one.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::cli::CmdError;

/// Where the ledger answers, when `--ledger` is not given.
pub(super) const LEDGER_ENV: &str = "STADO_CONTRIBUTION_LEDGER_URL";
/// The role whose item's `token` is the bearer Stado reports with; the
/// ledger's operator declares that bearer's SHA-256 for the product `stado`.
const BEARER_ROLE: &str = "stado-contribution-ledger";
const BEARER_FIELD: &str = "token";
const PRODUCT: &str = "stado";

/// The ledger `--ledger` names, else the one `STADO_CONTRIBUTION_LEDGER_URL`
/// declares; `None` when neither names one.
pub(super) fn named(flag: Option<&str>) -> Result<Option<String>, CmdError> {
    let named = match flag {
        Some(url) => Some(url.to_string()),
        None => match std::env::var(LEDGER_ENV) {
            Ok(url) => Some(url),
            Err(std::env::VarError::NotPresent) => None,
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err(CmdError::declaration(format!(
                    "{LEDGER_ENV} is not valid Unicode"
                )))
            }
        },
    };
    Ok(named
        .map(|url| url.trim().trim_end_matches('/').to_string())
        .filter(|url| !url.is_empty()))
}

/// One ledger, reached with Stado's reporter bearer.
pub(super) struct Ledger {
    pub(super) url: String,
    bearer: String,
    client: reqwest::Client,
}

impl Ledger {
    pub(super) async fn open(url: String) -> Result<Self, CmdError> {
        let bearer = crate::credential_store::read_string(BEARER_ROLE, BEARER_FIELD)
            .await
            .map_err(|error| {
                CmdError::click(format!(
                    "reading {BEARER_ROLE}#{BEARER_FIELD} failed: {error}"
                ))
            })?
            .map(|bearer| bearer.trim().to_string())
            .filter(|bearer| !bearer.is_empty())
            .ok_or_else(|| {
                CmdError::missing(format!(
                    "the credential store holds no {BEARER_ROLE}#{BEARER_FIELD}"
                ))
            })?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| {
                CmdError::click(format!("building the ledger client failed: {error}"))
            })?;
        Ok(Self {
            url,
            bearer,
            client,
        })
    }

    /// POST `body` to `route`: the ledger's answer, or its refusal in its own words.
    async fn post(&self, route: &str, body: &Value) -> Result<Value, String> {
        let target = format!("{}{route}", self.url);
        let response = crate::wait::send(
            crate::wait::Kind::Network,
            "the contribution ledger",
            self.client
                .post(&target)
                .bearer_auth(&self.bearer)
                .header("x-wisent-reporter", PRODUCT)
                .json(body),
        )
        .await
        .map_err(|error| format!("POST {target}: {error}"))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|error| format!("POST {target} answered {status} unreadably: {error}"))?;
        let answer: Value = serde_json::from_str(&text).map_err(|error| {
            format!("POST {target} answered {status} with no JSON ({error}): {text}")
        })?;
        if status.is_success() {
            return Ok(answer);
        }
        Err(
            match answer.pointer("/error/message").and_then(Value::as_str) {
                Some(message) => message.to_string(),
                None => format!("POST {target} answered {status}: {text}"),
            },
        )
    }

    /// Which of `ids` the ledger already holds from Stado, asked in one request.
    pub(super) async fn held(&self, ids: &[&str]) -> Result<BTreeSet<String>, CmdError> {
        let answer = self
            .post(
                "/api/v1/contributions/events/held",
                &json!({"source_event_ids": ids}),
            )
            .await
            .map_err(|cause| {
                CmdError::click(format!(
                    "asking the contribution ledger what it holds failed: {cause}"
                ))
            })?;
        let Some(held) = answer.get("held").and_then(Value::as_array) else {
            return Err(CmdError::click(format!(
                "the contribution ledger's answer names no held list: {answer}"
            )));
        };
        held.iter()
            .map(|id| {
                id.as_str().map(str::to_string).ok_or_else(|| {
                    CmdError::click(format!(
                        "the contribution ledger held an id that is not text: {id}"
                    ))
                })
            })
            .collect()
    }

    /// Record one event, idempotent by its `source_event_id`.
    pub(super) async fn record(&self, event: &Value) -> Result<Value, String> {
        self.post("/api/v1/contributions/events", event).await
    }
}
