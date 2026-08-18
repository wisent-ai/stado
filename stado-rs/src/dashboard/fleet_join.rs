//! Invite-authenticated enrollment: the machine being added adds itself.
//!
//! Three routes, all reachable by a machine that holds nothing but a one-line
//! invite code and no Stado credentials at all:
//!
//! * `GET  /join.sh`               — the bootstrap script, unauthenticated
//! * `GET  /api/fleet/invite/key`  — the fleet's PUBLIC key for this invite
//! * `POST /api/fleet/join`        — record the machine's pending request
//!
//! Key direction is fixed and one-way: the fleet dials the machine, so the
//! machine receives a public key for its `authorized_keys` and the private
//! half never leaves the operator's vault. Nothing here mints, reads or
//! forwards private key material.
//!
//! Authorization on the two API routes is the invite token and nothing else.
//! Operator credentials are never consulted and never accepted: a bearer that
//! is an operator session, or a loopback caller that the operator routes trust
//! implicitly, is refused exactly like an unknown code. These routes never
//! write the registry — approval does that, from the operator's side, through
//! the probing `fleet enroll` path.
//!
//! The invite lifecycle itself lives in [`crate::cli::fleet::invite`] and is
//! not reimplemented here: this module parses the token, reads the object
//! once (keeping the version it needs for a compare-and-swap spend), and asks
//! that module what the invite's status actually is.
//!
//! Refusals are uniform. Unknown, spent, revoked, expired, malformed, and
//! rate-limited all produce the same status, the same body, and the same
//! floor on elapsed time (`REFUSAL_FLOOR`), so a caller cannot use the
//! endpoint to learn which of those states a code is in, nor to enumerate
//! codes by timing.
//!
//! Rate limiting here is deliberately NOT `crate::rate_limit::RateLimiter`,
//! the shared limiter the dashboard exposes on `/api/rate-limit/consume`, and
//! it must not be "unified" with it later. That limiter (a) authenticates the
//! caller as a configured `RateLimitClient` from Skarbiec, which a machine
//! holding only an invite code cannot be, and (b) persists its window state to
//! the object store on every allowed consume — so wiring an unauthenticated
//! route into it converts a request flood into one object-store write per
//! request. That is the cost this limiter exists to bound, not a way to bound
//! it. The window below is process-local, checked before any store or vault
//! read, and costs one mutex.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::{json, Value};

use crate::cli::fleet::invite::{self, Invite};
use crate::queue::{JobStorage, StorageError};
use crate::targets::normalize_hostname;

use super::{http_status, json_dumps_sorted_compact, send_json, Request, Response};

/// The joining machine's report is a handful of short strings; anything
/// larger is a mistake or an attempt to make the dashboard allocate.
pub(super) const MAX_REQUEST_BYTES: usize = 4096;

/// The bootstrap script, embedded verbatim from the repository's
/// `deploy/join.sh` by `build.rs`. Empty means the build tree had no script.
const JOIN_SCRIPT: &str = include_str!(concat!(env!("OUT_DIR"), "/join.sh"));

/// Store prefix of the join requests these routes file.
const REQUESTS_PREFIX: &str = "enrollments/";
const STATUS_PENDING: &str = "pending";

/// Longest accepted value for any single reported string field.
const MAX_FIELD_BYTES: usize = 255;

/// Per-window allowances. One machine joining needs two requests, so the
/// per-token window leaves room for retries and nothing else.
const WINDOW: Duration = Duration::from_secs(60);
const MAX_PER_TOKEN: u32 = 12;
const MAX_PER_ADDRESS: u32 = 60;
/// Distinct rate-limit keys retained before the table refuses to grow, so a
/// spray of forged ids cannot turn the limiter into the memory leak.
const MAX_TRACKED_KEYS: usize = 4096;

/// Every refusal takes at least this long, measured from the start of the
/// request, so a rejected code cannot be classified by how fast it failed —
/// including the difference between "rate limited" (no I/O) and "no such
/// invite" (one store read).
const REFUSAL_FLOOR: Duration = Duration::from_millis(250);

/// The one refusal every failed authorization produces, kept identical to the
/// CLI's.
const REFUSAL: &str = "invite token is not usable";

/// Compared against when no digest could be loaded, so an unknown id costs
/// the same hash and the same comparison as a known one.
const ABSENT_DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn bearer(request: &Request) -> Option<&str> {
    request
        .header("authorization")
        .map(str::trim)
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// Token id and secret, owned, when the presented bearer has the right shape.
/// Shape failures are refusals like any other; the id is kept because the
/// limiter charges the request before anything else looks at it.
fn presented(request: &Request) -> Option<(String, String)> {
    let raw = bearer(request)?;
    invite::parse_token(raw)
        .ok()
        .map(|(id, secret)| (id.to_string(), secret.to_string()))
}

fn request_path(hostname: &str) -> String {
    format!("{REQUESTS_PREFIX}{hostname}.json")
}

// ---------------------------------------------------------------------------
// Process-local fixed window (see the module note on why the shared
// store-backed limiter is not used here)
// ---------------------------------------------------------------------------

struct Window {
    started: Instant,
    count: u32,
}

static WINDOWS: LazyLock<Mutex<HashMap<String, Window>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn within_limit(key: String, limit: u32, now: Instant) -> bool {
    let mut windows = WINDOWS.lock().expect("fleet-join rate-limit lock");
    windows.retain(|_, window| now.duration_since(window.started) < WINDOW);
    if windows.len() >= MAX_TRACKED_KEYS && !windows.contains_key(&key) {
        // Saturated by distinct keys: a new key waits out the window rather
        // than letting the table grow without bound.
        return false;
    }
    let window = windows.entry(key).or_insert(Window {
        started: now,
        count: u32::MIN,
    });
    window.count = window.count.saturating_add(u32::from(true));
    window.count <= limit
}

/// Charge one request against both buckets. Both are always charged, so a
/// caller cannot dodge the address bucket by rotating token ids.
fn accept_request(token_id: Option<&str>, peer: Option<IpAddr>) -> bool {
    let now = Instant::now();
    let address_key = match peer {
        Some(address) => format!("address:{address}"),
        None => "address:unknown".to_string(),
    };
    let token_key = match token_id {
        Some(id) => format!("token:{id}"),
        None => "token:malformed".to_string(),
    };
    let address_ok = within_limit(address_key, MAX_PER_ADDRESS, now);
    let token_ok = within_limit(token_key, MAX_PER_TOKEN, now);
    address_ok && token_ok
}

#[cfg(test)]
fn reset_limits() {
    WINDOWS.lock().expect("fleet-join rate-limit lock").clear();
}

// ---------------------------------------------------------------------------
// Invite verification
// ---------------------------------------------------------------------------

/// Why a route stopped. `Refused` is the single indistinguishable answer;
/// `Unavailable` is infrastructure, which says nothing about the token.
enum Denial {
    Refused,
    Unavailable(&'static str),
}

/// An invite that verified, with the version its document was read at so the
/// spend can be a compare-and-swap.
struct Accepted {
    invite: Invite,
    version: String,
}

/// Verify a presented token against its stored invite.
///
/// Every state — unknown id, wrong secret, unparsable document, revoked,
/// spent, exhausted, expired — collapses into `Denial::Refused` after the
/// same work: one store read, one digest, one constant-time comparison. There
/// is no early return between the read and the verdict, and the status itself
/// is decided by `invite::effective_status`, never re-derived here.
async fn verify(store: &JobStorage, id: &str, secret: &str) -> Result<Accepted, Denial> {
    let stored = match store.read_text_versioned(&invite::invite_path(id)).await {
        Ok(stored) => stored,
        Err(_) => return Err(Denial::Unavailable("enrollment store is unavailable")),
    };
    let (content, version) = match stored {
        Some(versioned) => (versioned.content, versioned.version),
        None => (String::new(), String::new()),
    };
    let parsed = serde_json::from_str::<Value>(&content)
        .ok()
        .and_then(|document| invite::parse_invite(&document).ok());

    // A missing invite still pays for a digest and a comparison; skipping
    // them would make "no such id" the fast answer.
    let expected = parsed
        .as_ref()
        .map(|invite| invite.secret_sha256.as_str())
        .unwrap_or(ABSENT_DIGEST);
    let secret_ok = invite::digests_match(expected, &invite::secret_digest(secret));
    let usable = parsed.as_ref().is_some_and(|invite| {
        invite.id == id && invite::effective_status(invite, Utc::now()) == invite::STATUS_OPEN
    });

    match parsed {
        Some(invite) if secret_ok & usable => Ok(Accepted { invite, version }),
        _ => Err(Denial::Refused),
    }
}

/// Consume one use, atomically against concurrent joins on the same code. A
/// lost race means the use went to another machine, which is an exhausted
/// code, which is the same refusal as any other.
async fn spend(store: &JobStorage, accepted: &Accepted) -> Result<Value, Denial> {
    let mut next = accepted.invite.clone();
    next.uses_spent = next.uses_spent.saturating_add(u64::from(true));
    if next.uses_spent >= next.uses_allowed {
        next.status = invite::STATUS_SPENT.to_string();
    }
    let document = invite::invite_document(&next);
    let body = serde_json::to_string_pretty(&document)
        .map_err(|_| Denial::Unavailable("enrollment store is unavailable"))?;
    match store
        .compare_and_swap_text(
            &invite::invite_path(&accepted.invite.id),
            &accepted.version,
            &body,
        )
        .await
    {
        Ok(_) => Ok(document),
        Err(StorageError::StorageConflict(_) | StorageError::NotFound(_)) => Err(Denial::Refused),
        Err(_) => Err(Denial::Unavailable("enrollment store is unavailable")),
    }
}

/// Put a spent use back when the request it paid for could not be recorded, so
/// a storage failure does not silently burn the owner's only code. Best
/// effort: if another writer has since moved the document on, the use stays
/// spent rather than being resurrected under someone else's write.
async fn refund(store: &JobStorage, accepted: &Accepted, spent: &Value) {
    let path = invite::invite_path(&accepted.invite.id);
    let Ok(Some(current)) = store.read_text_versioned(&path).await else {
        return;
    };
    if serde_json::from_str::<Value>(&current.content).ok().as_ref() != Some(spent) {
        return;
    }
    let restored = invite::invite_document(&accepted.invite);
    let Ok(body) = serde_json::to_string_pretty(&restored) else {
        return;
    };
    let _ = store
        .compare_and_swap_text(&path, &current.version, &body)
        .await;
}

// ---------------------------------------------------------------------------
// Responses
// ---------------------------------------------------------------------------

/// The single refusal, held to a fixed floor on elapsed time.
async fn refuse(started: Instant) -> Response {
    let elapsed = started.elapsed();
    if elapsed < REFUSAL_FLOOR {
        tokio::time::sleep(REFUSAL_FLOOR - elapsed).await;
    }
    send_json(http_status("401"), &json!({"error": REFUSAL}))
}

fn unavailable(message: &str) -> Response {
    send_json(http_status("503"), &json!({"error": message}))
}

async fn denied(started: Instant, denial: Denial) -> Response {
    match denial {
        Denial::Refused => refuse(started).await,
        Denial::Unavailable(message) => unavailable(message),
    }
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

/// `GET /join.sh` — the machine-side bootstrap script, verbatim from the
/// repository, unauthenticated. The script discloses nothing: the code is the
/// user's own argument to it. Never cached, so a re-issued script reaches the
/// next machine that runs the line.
pub(super) fn join_script() -> Response {
    if JOIN_SCRIPT.is_empty() {
        return unavailable(
            "join script unavailable: this build has no deploy/join.sh in its source tree",
        );
    }
    Response::new_with_headers(
        http_status("200"),
        "OK",
        "text/plain; charset=utf-8",
        JOIN_SCRIPT.as_bytes(),
        &[
            (
                "Cache-Control",
                "no-store, no-cache, must-revalidate".to_string(),
            ),
            ("Pragma", "no-cache".to_string()),
        ],
    )
}

/// `GET /api/fleet/invite/key` — the public half of the fleet's channel key
/// for this invite's target, plus the exact `authorized_keys` line to append.
/// Reads no registry, writes nothing, spends nothing.
pub(super) async fn invite_key(store: &JobStorage, request: &Request) -> Response {
    let started = Instant::now();
    let token = presented(request);
    if !accept_request(token.as_ref().map(|(id, _)| id.as_str()), request.peer) {
        return refuse(started).await;
    }
    let Some((id, secret)) = token else {
        return refuse(started).await;
    };
    if !request.body.is_empty() {
        return refuse(started).await;
    }
    let accepted = match verify(store, &id, &secret).await {
        Ok(accepted) => accepted,
        Err(denial) => return denied(started, denial).await,
    };

    let target = accepted.invite.target_name;
    let item = crate::cli::fleet::key::item_id(&target);
    let client = match crate::cli::fleet::key::configured_client() {
        Ok(client) => client,
        Err(_) => return unavailable("enrollment key store is unavailable"),
    };
    let stored = match client.read_string(&item, "public_key").await {
        Ok(Some(value)) if !value.trim().is_empty() => value,
        Ok(_) => return unavailable("enrollment key is not available for this invite"),
        Err(_) => return unavailable("enrollment key store is unavailable"),
    };
    // `ssh-keygen` leaves its own comment on the key, so the stored value is
    // "<type> <blob> [comment]". The machine appends one line naming the
    // credential item it came from; repeating ssh-keygen's comment there
    // would make that line self-describing twice and match nothing an
    // operator later greps for.
    let mut fields = stored.split_whitespace();
    let public_key = match (fields.next(), fields.next()) {
        (Some(kind), Some(blob)) => format!("{kind} {blob}"),
        _ => return unavailable("enrollment key is not available for this invite"),
    };
    send_json(
        http_status("200"),
        &json!({
            "target_name": target,
            "public_key": public_key,
            "authorized_keys_line":
                crate::cli::fleet::key::authorized_keys_line(&public_key, &item),
        }),
    )
}

/// The machine's self-report. Extra keys are tolerated so the bootstrap
/// script can report more without a lockstep release; `ssh_listening` is the
/// observation it makes today, because a laptop whose owner has not yet
/// granted Remote Login must still be able to file its request.
struct Report {
    hostname: String,
    os: String,
    arch: String,
    destination: String,
    fingerprint: String,
    ssh_listening: Option<bool>,
}

fn parse_report(body: &[u8]) -> Result<Report, &'static str> {
    let document: Value = serde_json::from_slice(body).map_err(|_| "request body is not JSON")?;
    let required = |name: &str| -> Result<String, &'static str> {
        let value = document
            .get(name)
            .and_then(Value::as_str)
            .ok_or("join report is missing a required field")?
            .trim()
            .to_string();
        if value.is_empty() || value.len() > MAX_FIELD_BYTES {
            return Err("join report field is empty or too long");
        }
        Ok(value)
    };
    // A machine with neither ssh-keygen nor openssl cannot fingerprint the
    // key it just installed. That is reported empty, never fabricated.
    let fingerprint = document
        .get("installed_key_fingerprint")
        .and_then(Value::as_str)
        .ok_or("join report is missing a required field")?
        .trim()
        .to_string();
    if fingerprint.len() > MAX_FIELD_BYTES {
        return Err("join report field is empty or too long");
    }
    Ok(Report {
        hostname: required("hostname")?,
        os: required("os")?,
        arch: required("arch")?,
        destination: required("destination")?,
        fingerprint,
        ssh_listening: document.get("ssh_listening").and_then(Value::as_bool),
    })
}

/// A reported hostname becomes an object key, so it is held to what a machine
/// name can be: no separators, no traversal, nothing that could address a
/// different part of the store.
fn valid_hostname(hostname: &str) -> bool {
    !hostname.is_empty()
        && hostname.len() <= MAX_FIELD_BYTES
        && hostname
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.')
        && !hostname.starts_with('.')
        && !hostname.contains("..")
}

/// `POST /api/fleet/join` — record the machine's pending enrollment request
/// and consume one use of the invite. Writes only under `enrollments/`; the
/// registry is untouched until an operator approves, and approval re-probes
/// the machine over the channel this request names.
pub(super) async fn join(store: &JobStorage, request: &Request) -> Response {
    let started = Instant::now();
    let token = presented(request);
    if !accept_request(token.as_ref().map(|(id, _)| id.as_str()), request.peer) {
        return refuse(started).await;
    }
    let content_type = request
        .header("content-type")
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .unwrap_or_default();
    if content_type != "application/json" {
        return send_json(
            http_status("400"),
            &json!({"error": "join report requires Content-Type: application/json"}),
        );
    }
    if request.body.len() > MAX_REQUEST_BYTES {
        return send_json(
            http_status("413"),
            &json!({"error": "join report is too large"}),
        );
    }
    let report = match parse_report(&request.body) {
        Ok(report) => report,
        Err(message) => return send_json(http_status("400"), &json!({"error": message})),
    };
    let hostname = normalize_hostname(&report.hostname);
    if !valid_hostname(&hostname) {
        return send_json(
            http_status("400"),
            &json!({"error": "join report hostname is not a usable machine name"}),
        );
    }
    let Some((id, secret)) = token else {
        return refuse(started).await;
    };
    let accepted = match verify(store, &id, &secret).await {
        Ok(accepted) => accepted,
        Err(denial) => return denied(started, denial).await,
    };

    // A machine whose request was already decided is never silently reopened
    // by a code; only a still-pending request is replaced.
    let path = request_path(&hostname);
    let existing = match store.read_text_versioned(&path).await {
        Ok(existing) => existing,
        Err(_) => return unavailable("enrollment store is unavailable"),
    };
    if let Some(current) = &existing {
        let status = serde_json::from_str::<Value>(&current.content)
            .ok()
            .and_then(|document| {
                document
                    .get("status")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        if status != STATUS_PENDING {
            return send_json(
                http_status("409"),
                &json!({
                    "error": format!(
                        "machine '{hostname}' already has a '{status}' enrollment request"
                    )
                }),
            );
        }
    }

    let spent = match spend(store, &accepted).await {
        Ok(spent) => spent,
        Err(denial) => return denied(started, denial).await,
    };

    let mut document = crate::cli::fleet::enroll::build_invited_request(
        &hostname,
        &report.os,
        &report.arch,
        &accepted.invite.target_name,
        &report.destination,
        &accepted.invite.id,
        &report.fingerprint,
    );
    if let Some(listening) = report.ssh_listening {
        document["ssh_listening"] = json!(listening);
    }
    let body = json_dumps_sorted_compact(&document);
    let recorded = match &existing {
        Some(current) => store
            .compare_and_swap_text(&path, &current.version, &body)
            .await
            .map(|_| true),
        None => store.create_text_if_absent(&path, &body).await,
    };
    match recorded {
        Ok(true) => {}
        Ok(false) | Err(StorageError::StorageConflict(_)) => {
            refund(store, &accepted, &spent).await;
            return send_json(
                http_status("409"),
                &json!({"error": format!("machine '{hostname}' is already enrolling")}),
            );
        }
        Err(_) => {
            refund(store, &accepted, &spent).await;
            return unavailable("enrollment store is unavailable");
        }
    }
    send_json(
        http_status("200"),
        &json!({
            "status": STATUS_PENDING,
            "hostname": hostname,
            "target_name": accepted.invite.target_name,
            "next_step": format!("an operator approves with: stado fleet approve {hostname}"),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_allows_the_budget_then_refuses() {
        reset_limits();
        let now = Instant::now();
        for _ in u32::MIN..MAX_PER_TOKEN {
            assert!(within_limit("token:test".to_string(), MAX_PER_TOKEN, now));
        }
        assert!(!within_limit("token:test".to_string(), MAX_PER_TOKEN, now));
        // A later window forgets the old count.
        let later = now + WINDOW + Duration::from_secs(u64::from(true));
        assert!(within_limit("token:test".to_string(), MAX_PER_TOKEN, later));
        reset_limits();
    }

    #[test]
    fn hostnames_stay_inside_the_enrollment_prefix() {
        assert!(valid_hostname("worker-box.local"));
        assert!(!valid_hostname("../registry"));
        assert!(!valid_hostname("box/../../etc"));
        assert!(!valid_hostname(""));
    }

    #[test]
    fn report_requires_its_fields_and_tolerates_later_ones() {
        let body = br#"{"hostname":"box","os":"Darwin","arch":"arm64",
            "destination":"user@box","installed_key_fingerprint":"SHA256:x",
            "ssh_listening":false,"future":"ignored"}"#;
        let report = parse_report(body).expect("valid report");
        assert_eq!(report.hostname, "box");
        assert_eq!(report.ssh_listening, Some(false));
        // The fingerprint may be empty on a machine with no digest tool.
        let bare = br#"{"hostname":"box","os":"Darwin","arch":"arm64",
            "destination":"user@box","installed_key_fingerprint":""}"#;
        assert_eq!(parse_report(bare).expect("valid report").fingerprint, "");
        let missing = br#"{"hostname":"box","os":"Darwin","arch":"arm64"}"#;
        assert!(parse_report(missing).is_err());
    }
}
