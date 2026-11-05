//! One inference call bought without an account.
//!
//! A deployment's operator declares an offer: the price per million prompt
//! and generated tokens, the most tokens one call may take, how long a quote
//! holds, and the payment methods that settle a quote. No method is built in:
//! each is a verification endpoint the operator names, asked whether a
//! payment reference settled a quote for its amount, with a bearer from a
//! vault item when the endpoint needs one.
//!
//! The gateway answers a request without a key `402 Payment Required` with a
//! quote (its id, the most the call can cost, the methods, the expiry). The
//! caller pays by any method and posts the reference to `/v1/pay`; the
//! gateway asks the method's endpoint, and when it states the quote settled
//! for at least its amount, mints a key good for one call and keeps only its
//! SHA-256. A payment reference is accepted once, a key is spent once (both
//! by create-only writes, so two gateways cannot accept the same twice), and
//! the call's settlement — what was paid, the tokens the engine stated, what
//! they cost at the offer's price and the refund due — is written beside the
//! payment when the call returns. A call the engine fails is settled with
//! the whole payment due back; a call whose answer states no usage keeps its
//! cost unread, never zero.
//!
//! Everything lives under `inference/pay-per-call/<deployment>/` in the
//! fleet store: `offer.json`, `quotes/<id>.json`, `references/<method>/<sha256
//! of the reference>.json`, `paid/<key sha256>.json`, `spent/<key
//! sha256>.json` and `settlements/<quote>.json`.

use serde::{Deserialize, Serialize};

use super::callers::{key_hash, Price};
use crate::queue::{JobStorage, Listing, StorageError};

pub const PREFIX: &str = "inference/pay-per-call/";

/// What a one-time key starts with, so the gateway tells it from a declared
/// caller's key without a store read.
pub const KEY_PREFIX: &str = "ppc-";

/// One way a quote can be paid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Method {
    /// The name a payer posts with the reference.
    pub name: String,
    /// The endpoint asked whether a reference settled a quote: it is sent
    /// `POST {"quote", "deployment", "usd", "reference"}` and answers
    /// `{"settled": bool, "usd": number}`.
    pub verify_url: String,
    /// The vault item whose `token` field is sent as the endpoint's bearer.
    pub credential_item: Option<String>,
}

/// What one deployment sells one call for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Offer {
    pub deployment: String,
    pub price: Price,
    /// The most prompt tokens one call may carry.
    pub max_prompt_tokens: u64,
    /// The most tokens one call may generate; sent to the engine as the
    /// request's `max_tokens` when the request asks for more or names none.
    pub max_generated_tokens: u64,
    /// Seconds a quote can be paid in.
    pub quote_seconds: u64,
    pub methods: Vec<Method>,
    pub offered_at: String,
}

impl Offer {
    /// The most one call can cost: every token the offer allows at its price.
    pub fn bound_usd(&self) -> f64 {
        self.max_prompt_tokens as f64 / 1e6 * self.price.usd_per_million_prompt
            + self.max_generated_tokens as f64 / 1e6 * self.price.usd_per_million_generated
    }
}

/// A price stated to one prospective payer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quote {
    pub id: String,
    pub deployment: String,
    /// What the payer must pay: the offer's bound.
    pub usd: f64,
    pub price: Price,
    pub max_prompt_tokens: u64,
    pub max_generated_tokens: u64,
    pub methods: Vec<String>,
    pub issued_at: String,
    pub expires_at: String,
}

/// A settled payment, keyed by the one-time key it minted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Payment {
    pub quote: String,
    pub deployment: String,
    pub method: String,
    /// SHA-256 of the payment reference; the reference itself is not kept.
    pub reference_sha256: String,
    /// What the method's endpoint stated was paid.
    pub usd_paid: f64,
    pub key_sha256: String,
    pub paid_at: String,
}

/// What one paid call did and cost, written when it returns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settlement {
    pub quote: String,
    pub deployment: String,
    pub method: String,
    pub usd_paid: f64,
    /// `served` when the engine answered below 400, else `failed`.
    pub outcome: String,
    /// The engine's HTTP status, or none when it never answered.
    pub engine_status: Option<u16>,
    pub prompt_tokens: Option<u64>,
    pub generated_tokens: Option<u64>,
    pub seconds: f64,
    /// The stated tokens at the offer's price; none when the answer stated
    /// no usage.
    pub cost_usd: Option<f64>,
    /// What is owed back: the whole payment for a failed call, the payment
    /// less the cost for a served one; none when the cost is unread.
    pub refund_due_usd: Option<f64>,
    /// Set when the stated tokens cost more than was paid (a prompt above
    /// the offer's bound): the amount not covered.
    pub unpaid_usd: Option<f64>,
    pub settled_at: String,
}

impl Settlement {
    /// The settlement of a call: `stated` is the engine's (prompt,
    /// generated) tokens when its answer stated them.
    pub fn of(
        payment: &Payment,
        price: &Price,
        engine_status: Option<u16>,
        stated: Option<(u64, u64)>,
        seconds: f64,
    ) -> Self {
        let served = engine_status.is_some_and(|status| status < 400);
        let cost = stated.map(|(prompt, generated)| {
            prompt as f64 / 1e6 * price.usd_per_million_prompt
                + generated as f64 / 1e6 * price.usd_per_million_generated
        });
        let (refund, unpaid) = match (served, cost) {
            (false, _) => (Some(payment.usd_paid), None),
            (true, Some(cost)) if cost > payment.usd_paid => {
                (Some(0.0), Some(cost - payment.usd_paid))
            }
            (true, Some(cost)) => (Some(payment.usd_paid - cost), None),
            (true, None) => (None, None),
        };
        Settlement {
            quote: payment.quote.clone(),
            deployment: payment.deployment.clone(),
            method: payment.method.clone(),
            usd_paid: payment.usd_paid,
            outcome: if served { "served" } else { "failed" }.to_string(),
            engine_status,
            prompt_tokens: stated.map(|(prompt, _)| prompt),
            generated_tokens: stated.map(|(_, generated)| generated),
            seconds,
            cost_usd: cost,
            refund_due_usd: refund,
            unpaid_usd: unpaid,
            settled_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// What the call earned: what was paid less what is owed back; none
    /// while the refund is unread.
    pub fn earned_usd(&self) -> Option<f64> {
        self.refund_due_usd.map(|refund| self.usd_paid - refund)
    }
}

fn path(deployment: &str, rest: &str) -> String {
    format!("{PREFIX}{deployment}/{rest}")
}

fn encode<T: Serialize>(what: &str, value: &T) -> Result<String, StorageError> {
    serde_json::to_string_pretty(value)
        .map_err(|error| StorageError::Other(format!("{what} cannot be encoded: {error}")))
}

async fn read<T: for<'de> Deserialize<'de>>(
    store: &JobStorage,
    key: &str,
    what: &str,
) -> Result<Option<T>, StorageError> {
    let Some(text) = store.download_text(key).await? else {
        return Ok(None);
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|error| StorageError::Other(format!("{key} is not {what}: {error}")))
}

pub async fn offer(store: &JobStorage, deployment: &str) -> Result<Option<Offer>, StorageError> {
    read(
        store,
        &path(deployment, "offer.json"),
        "a pay-per-call offer",
    )
    .await
}

pub async fn save_offer(store: &JobStorage, offer: &Offer) -> Result<(), StorageError> {
    store
        .upload_text(
            &path(&offer.deployment, "offer.json"),
            &encode("an offer", offer)?,
        )
        .await
}

/// Withdraw the offer; `false` when none was declared. Quotes already
/// paid keep their keys and settlements.
pub async fn withdraw_offer(store: &JobStorage, deployment: &str) -> Result<bool, StorageError> {
    let key = path(deployment, "offer.json");
    if store.download_text(&key).await?.is_none() {
        return Ok(false);
    }
    store.delete_blob(&key).await?;
    Ok(true)
}

/// State a new quote from the offer and keep it.
pub async fn quote(store: &JobStorage, offer: &Offer) -> Result<Quote, StorageError> {
    let now = chrono::Utc::now();
    let seconds = i64::try_from(offer.quote_seconds).unwrap_or(i64::MAX);
    let quote = Quote {
        id: uuid::Uuid::new_v4().simple().to_string(),
        deployment: offer.deployment.clone(),
        usd: offer.bound_usd(),
        price: offer.price,
        max_prompt_tokens: offer.max_prompt_tokens,
        max_generated_tokens: offer.max_generated_tokens,
        methods: offer
            .methods
            .iter()
            .map(|method| method.name.clone())
            .collect(),
        issued_at: now.to_rfc3339(),
        expires_at: (now + chrono::Duration::seconds(seconds)).to_rfc3339(),
    };
    store
        .upload_text(
            &path(&offer.deployment, &format!("quotes/{}.json", quote.id)),
            &encode("a quote", &quote)?,
        )
        .await?;
    Ok(quote)
}

pub async fn load_quote(
    store: &JobStorage,
    deployment: &str,
    id: &str,
) -> Result<Option<Quote>, StorageError> {
    // A quote id is a hex uuid; anything else names no kept quote.
    if id.is_empty() || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(None);
    }
    read(
        store,
        &path(deployment, &format!("quotes/{id}.json")),
        "a quote",
    )
    .await
}

/// Claim a payment reference for `quote`; `false` when the same reference
/// was already accepted, for this quote or another.
pub async fn claim_reference(
    store: &JobStorage,
    deployment: &str,
    method: &str,
    reference: &str,
    quote: &str,
) -> Result<bool, StorageError> {
    let key = path(
        deployment,
        &format!("references/{method}/{}.json", key_hash(reference)),
    );
    store
        .create_text_if_absent(&key, &serde_json::json!({ "quote": quote }).to_string())
        .await
}

/// Keep a settled payment and return the one-time key it minted.
pub async fn mint(
    store: &JobStorage,
    quote: &Quote,
    method: &str,
    reference: &str,
    usd_paid: f64,
) -> Result<String, StorageError> {
    let key = format!(
        "{KEY_PREFIX}{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let payment = Payment {
        quote: quote.id.clone(),
        deployment: quote.deployment.clone(),
        method: method.to_string(),
        reference_sha256: key_hash(reference),
        usd_paid,
        key_sha256: key_hash(&key),
        paid_at: chrono::Utc::now().to_rfc3339(),
    };
    store
        .upload_text(
            &path(
                &quote.deployment,
                &format!("paid/{}.json", payment.key_sha256),
            ),
            &encode("a payment", &payment)?,
        )
        .await?;
    Ok(key)
}

/// The payment `key` minted, when it minted one.
pub async fn payment_for(
    store: &JobStorage,
    deployment: &str,
    key: &str,
) -> Result<Option<Payment>, StorageError> {
    read(
        store,
        &path(deployment, &format!("paid/{}.json", key_hash(key))),
        "a payment",
    )
    .await
}

/// Spend a payment's key; `false` when it was already spent.
pub async fn spend(store: &JobStorage, payment: &Payment) -> Result<bool, StorageError> {
    store
        .create_text_if_absent(
            &path(
                &payment.deployment,
                &format!("spent/{}.json", payment.key_sha256),
            ),
            &serde_json::json!({ "spent_at": chrono::Utc::now().to_rfc3339() }).to_string(),
        )
        .await
}

pub async fn save_settlement(
    store: &JobStorage,
    settlement: &Settlement,
) -> Result<(), StorageError> {
    store
        .upload_text(
            &path(
                &settlement.deployment,
                &format!("settlements/{}.json", settlement.quote),
            ),
            &encode("a settlement", settlement)?,
        )
        .await
}

/// Every settlement of `deployment` at or after `since` (RFC 3339).
pub async fn settlements(
    store: &JobStorage,
    deployment: &str,
    since: &str,
) -> Result<Vec<Settlement>, StorageError> {
    let mut kept = Vec::new();
    for key in store
        .list_paths(&path(deployment, "settlements/"), Listing::Every)
        .await?
    {
        if let Some(settlement) = read::<Settlement>(store, &key, "a settlement").await? {
            if settlement.settled_at.as_str() >= since {
                kept.push(settlement);
            }
        }
    }
    kept.sort_by(|left, right| left.settled_at.cmp(&right.settled_at));
    Ok(kept)
}
