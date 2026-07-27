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
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
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
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
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
        None => Ok((None, BucketState { tokens: max_tokens, refilled_at: now() })),
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
    let Some(store) = store_or_none().await else { return };
    acquire(&store, RATE_OBJECT, MAX_TOKENS, REFILL_PER_SECOND, f64::from(n), timeout).await;
}

/// Block until n commit-tokens are free (HF 128 commits / hour / repo).
/// Python `wait_for_hf_commit_token(n=1, timeout=3600.0)`.
pub async fn wait_for_hf_commit_token(n: u32, timeout: Option<Duration>) {
    let Some(store) = store_or_none().await else { return };
    acquire(&store, COMMIT_OBJECT, COMMIT_MAX, COMMIT_REFILL_PER_SECOND, f64::from(n), timeout).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::local_file::LocalBackend;
    use std::sync::Arc;

    fn store() -> (tempfile::TempDir, JobStorage) {
        let dir = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(dir.path().to_str().unwrap()).unwrap();
        (dir, JobStorage::with_backend(Arc::new(backend), "local"))
    }

    #[test]
    fn refill_arithmetic() {
        let state = BucketState { tokens: 10.0, refilled_at: 100.0 };
        // 30s at 2 tok/s -> +60.
        let out = refill(state, 100.0, 2.0, 130.0);
        assert_eq!(out.tokens, 70.0);
        assert_eq!(out.refilled_at, 130.0);
        // Clamped at the cap.
        let out = refill(BucketState { tokens: 90.0, refilled_at: 100.0 }, 100.0, 2.0, 130.0);
        assert_eq!(out.tokens, 100.0);
        // Clock skew backwards -> no negative refill.
        let out = refill(BucketState { tokens: 50.0, refilled_at: 100.0 }, 100.0, 2.0, 90.0);
        assert_eq!(out.tokens, 50.0);
    }

    #[test]
    fn wait_arithmetic() {
        // deficit 2 at 4 tok/s -> 0.5s (above the 0.5 floor), plus jitter.
        assert_eq!(deficit_wait(2.0, 4.0, 0.25), 0.75);
        // Tiny deficit clamps up to the 0.5 base.
        assert_eq!(deficit_wait(0.1, 4.0, 0.0), 0.5);
        // Huge deficit clamps down to the 30s cap.
        assert_eq!(deficit_wait(1000.0, 1.0, 0.0), 30.0);
        // Error backoff: 0.5 * 2**attempt, exponent capped at 5, total at 30.
        assert_eq!(error_wait(1, 0.0), 1.0);
        assert_eq!(error_wait(5, 0.0), 16.0);
        assert_eq!(error_wait(10, 0.0), 16.0);
        assert_eq!(error_wait(20, 0.5), 16.5);
    }

    #[tokio::test]
    async fn acquire_initializes_full_and_deducts() {
        let (_dir, store) = store();
        acquire(&store, RATE_OBJECT, 4.0, 0.0, 1.0, Some(Duration::from_secs(5))).await;
        let vt = store.read_text_versioned(RATE_OBJECT).await.unwrap().unwrap();
        let state: BucketState = serde_json::from_str(&vt.content).unwrap();
        assert_eq!(state.tokens, 3.0);
    }

    #[tokio::test]
    async fn cas_conflict_retries_and_eventually_acquires() {
        let (_dir, store) = store();
        // Initialize the bucket.
        acquire(&store, RATE_OBJECT, 4.0, 0.0, 1.0, Some(Duration::from_secs(5))).await;

        // Lose the race deterministically: read the version, overwrite the
        // blob externally, then try to deduct against the stale version.
        let vt = store.read_text_versioned(RATE_OBJECT).await.unwrap().unwrap();
        store.upload_text(RATE_OBJECT, r#"{"tokens": 4.0, "refilled_at": 0.0}"#).await.unwrap();
        let err = try_acquire_stale(&store, &vt.version).await.unwrap_err();
        assert!(matches!(err, StorageError::StorageConflict(_)), "{err:?}");

        // The retry path (fresh read) succeeds — this is what acquire's
        // loop does after a StorageConflict backoff.
        acquire(&store, RATE_OBJECT, 4.0, 0.0, 1.0, Some(Duration::from_secs(5))).await;
        let vt = store.read_text_versioned(RATE_OBJECT).await.unwrap().unwrap();
        let state: BucketState = serde_json::from_str(&vt.content).unwrap();
        assert_eq!(state.tokens, 3.0);
    }

    /// Single deduct against a caller-supplied (stale) version — the CAS
    /// half of try_acquire_once, exposed so the test can force a conflict.
    async fn try_acquire_stale(store: &JobStorage, version: &str) -> Result<(), StorageError> {
        let mut state: BucketState =
            serde_json::from_str(&store.read_text_versioned(RATE_OBJECT).await?.unwrap().content)?;
        state.tokens -= 1.0;
        store.compare_and_swap_text(RATE_OBJECT, version, &serde_json::to_string(&state)?).await?;
        Ok(())
    }

    #[tokio::test]
    async fn concurrent_acquires_never_overspend() {
        let (_dir, store) = store();
        let mut handles = Vec::new();
        for _ in 0..4 {
            let store = store.clone();
            handles.push(tokio::spawn(async move {
                // No refill: exactly 4 tokens exist for 4 competitors; any
                // CAS conflict must retry rather than overspend.
                acquire(&store, RATE_OBJECT, 4.0, 0.0, 1.0, Some(Duration::from_secs(30))).await;
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }
        let vt = store.read_text_versioned(RATE_OBJECT).await.unwrap().unwrap();
        let state: BucketState = serde_json::from_str(&vt.content).unwrap();
        assert_eq!(state.tokens, 0.0);
    }

    #[tokio::test]
    async fn timeout_falls_through_instead_of_blocking() {
        let (_dir, store) = store();
        // Drain the bucket with a zero-refill rate so every further
        // acquire lands in the deficit branch.
        acquire(&store, RATE_OBJECT, 1.0, 0.0, 1.0, Some(Duration::from_secs(5))).await;
        // Python checks the deadline BEFORE sleeping, so a zero timeout
        // does exactly one deficit probe and falls through (the caller
        // proceeds and retries on its own 429) instead of hard-blocking
        // on infra failure.
        let start = Instant::now();
        acquire(&store, RATE_OBJECT, 1.0, 0.0, 1.0, Some(Duration::ZERO)).await;
        assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());
    }
}
