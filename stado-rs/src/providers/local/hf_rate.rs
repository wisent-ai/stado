//! GCS-backed global token buckets for HuggingFace API rate limits.
//!
//! Port of `stado/providers/local/hf_rate.py`.
//!
//! HF enforces two separate caps that a multi-agent fleet hits:
//!   - 1000 requests / 5-minute window (account-wide)
//!   - 128 repository commits / hour (per repo) — the figure HF returns in
//!     the 429 commit-rate body; confirmed live on wisent-ai/activations
//!     2026-05-24 when per-chunk uploads from 15 concurrent jobs blew past
//!     it.
//!
//! Each cap is modeled as its own shared token bucket in a small storage
//! object, updated with generation-match atomic CAS so every agent
//! coordinates on one counter. Callers block until a token is free, then
//! proceed — i.e. "if under the cap upload, else wait until the fleet is
//! under the cap".
//!
//! Usage:
//!     wait_for_hf_commit_token(1, Some(Duration::from_secs(3600))).await;
//!     api.upload_folder(...)       // then commit
//!
//! Python reads/writes raw GCS blobs; here the same objects go through
//! [`JobStorage::read_text_versioned`] / [`JobStorage::compare_and_swap_text`]
//! (and `create_text_if_absent` for the Python `if_generation_match=0`
//! first-write), so any backend works.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::queue::{JobStorage, StorageError};

// Request bucket: HF 1000 requests / 5-minute window.
pub const RATE_OBJECT: &str = "hf_rate/tokens.json";
pub const MAX_TOKENS: f64 = 1000.0;
pub const REFILL_PER_SECOND: f64 = 200.0 / 60.0; // 200 tokens/min
                                                 // Commit bucket: HF 128 commits / hour per repo (HF's own 429 figure).
pub const COMMIT_OBJECT: &str = "hf_rate/commit_tokens.json";
pub const COMMIT_MAX: f64 = 128.0;
pub const COMMIT_REFILL_PER_SECOND: f64 = 128.0 / 3600.0; // 128 commits/hour
const POLL_BACKOFF_BASE: f64 = 0.5;
const POLL_BACKOFF_MAX: f64 = 30.0;

/// Python's default timeouts on the public waiters.
pub const DEFAULT_TOKEN_TIMEOUT: Duration = Duration::from_secs(600);
pub const DEFAULT_COMMIT_TIMEOUT: Duration = Duration::from_secs(3600);

/// The shared bucket document: {"tokens": float, "refilled_at": epoch_s}.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BucketState {
    pub tokens: f64,
    pub refilled_at: f64,
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Python `_refill`: elapsed-time token replenishment, clamped to the cap.
/// `now` is explicit so the arithmetic is testable offline.
pub fn refill(state: BucketState, max_tokens: f64, refill_per_sec: f64, now: f64) -> BucketState {
    let elapsed = (now - state.refilled_at).max(0.0);
    BucketState {
        tokens: (state.tokens + elapsed * refill_per_sec).min(max_tokens),
        refilled_at: now,
    }
}

fn jitter() -> f64 {
    // Python adds random.uniform(0, _POLL_BACKOFF_BASE). The port has no
    // rand crate in its dependency set; sub-second clock nanos spread
    // concurrent pollers well enough for anti-herd jitter.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos % 1000) as f64 / 1000.0 * POLL_BACKOFF_BASE
}

/// Pure: deficit-driven wait (Python the `wait = deficit / refill` branch),
/// clamped to [0.5, 30] plus jitter.
pub fn deficit_wait(deficit: f64, refill_per_sec: f64, jitter: f64) -> f64 {
    (deficit / refill_per_sec).clamp(POLL_BACKOFF_BASE, POLL_BACKOFF_MAX) + jitter
}

/// Pure: storage-error backoff (Python the `except` branch):
/// 0.5 * 2**min(attempt, 5), capped at 30, plus jitter.
pub fn error_wait(attempt: u32, jitter: f64) -> f64 {
    (POLL_BACKOFF_BASE * 2f64.powi(attempt.min(5) as i32)).min(POLL_BACKOFF_MAX) + jitter
}

/// Python `_read_state`: (version, state). A missing object yields the
/// "initialize full" state with no version — the first write then goes
/// through create-if-absent (Python `if_generation_match: 0`), which is
/// race-safe: concurrent inits both write a full bucket; whichever lands
/// second is fine.
async fn read_state(
    store: &JobStorage,
    obj: &str,
    max_tokens: f64,
) -> Result<(Option<String>, BucketState), StorageError> {
    match store.read_text_versioned(obj).await? {
        None => Ok((
            None,
            BucketState {
                tokens: max_tokens,
                refilled_at: now(),
            },
        )),
        Some(vt) => Ok((Some(vt.version), serde_json::from_str(&vt.content)?)),
    }
}

enum Step {
    Acquired,
    Deficit(f64),
}

/// One read-refill-deduct round of `_acquire`. Errors (including a lost
/// CAS race, [`StorageError::StorageConflict`]) propagate so the caller can
/// apply Python's broad-`except` backoff uniformly.
async fn try_acquire_once(
    store: &JobStorage,
    obj: &str,
    max_tokens: f64,
    refill_per_sec: f64,
    n: f64,
) -> Result<Step, StorageError> {
    let (version, state) = read_state(store, obj, max_tokens).await?;
    let mut state = refill(state, max_tokens, refill_per_sec, now());
    if state.tokens < n {
        return Ok(Step::Deficit(n - state.tokens));
    }
    state.tokens -= n;
    let body = serde_json::to_string(&state)?;
    match version {
        Some(version) => {
            store.compare_and_swap_text(obj, &version, &body).await?;
        }
        None => {
            if !store.create_text_if_absent(obj, &body).await? {
                return Err(StorageError::StorageConflict(format!(
                    "concurrent initialization of {obj}"
                )));
            }
        }
    }
    Ok(Step::Acquired)
}

/// Block until n tokens in the named bucket are available + atomically
/// deducted. Python `_acquire`. Best-effort: on timeout it falls through
/// so the caller proceeds (and retries on its own 429) rather than
/// hard-blocking on infra failure.
pub async fn acquire(
    store: &JobStorage,
    obj: &str,
    max_tokens: f64,
    refill_per_sec: f64,
    n: f64,
    timeout: Option<Duration>,
) {
    let deadline = timeout.map(|t| Instant::now() + t);
    let mut attempt: u32 = 0;
    loop {
        let wait = match try_acquire_once(store, obj, max_tokens, refill_per_sec, n).await {
            Ok(Step::Acquired) => return,
            Ok(Step::Deficit(deficit)) => deficit_wait(deficit, refill_per_sec, jitter()),
            Err(_) => {
                attempt += 1;
                error_wait(attempt, jitter())
            }
        };
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return;
        }
        tokio::time::sleep(Duration::from_secs_f64(wait)).await;
    }
}

/// Resolve the storage facade; None when the backend cannot be built
/// (Python `_get_bucket` returning None -> best-effort no-op).
async fn store_or_none() -> Option<JobStorage> {
    JobStorage::new().await.ok()
}

/// Block until n request-tokens are free (HF 1000 requests / 5 min).
/// Python `wait_for_hf_token(n=1, timeout=600.0)` — pass
/// [`DEFAULT_TOKEN_TIMEOUT`] for the Python default; None = block forever.
pub async fn wait_for_hf_token(n: u32, timeout: Option<Duration>) {
    let Some(store) = store_or_none().await else {
        return;
    };
    acquire(
        &store,
        RATE_OBJECT,
        MAX_TOKENS,
        REFILL_PER_SECOND,
        f64::from(n),
        timeout,
    )
    .await;
}

/// Block until n commit-tokens are free (HF 128 commits / hour / repo).
/// Python `wait_for_hf_commit_token(n=1, timeout=3600.0)`.
pub async fn wait_for_hf_commit_token(n: u32, timeout: Option<Duration>) {
    let Some(store) = store_or_none().await else {
        return;
    };
    acquire(
        &store,
        COMMIT_OBJECT,
        COMMIT_MAX,
        COMMIT_REFILL_PER_SECOND,
        f64::from(n),
        timeout,
    )
    .await;
}
