//! The process-local fixed window that charges every request before any
//! store or vault read.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Per-window allowances. One machine joining needs two requests, so the
/// per-token window leaves room for retries and nothing else.
const WINDOW: Duration = Duration::from_secs(60);
const MAX_PER_TOKEN: u32 = 12;
const MAX_PER_ADDRESS: u32 = 60;
/// Distinct rate-limit keys retained before the table refuses to grow, so a
/// spray of forged ids cannot turn the limiter into the memory leak.
const MAX_TRACKED_KEYS: usize = 4096;

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
pub(super) fn accept_request(token_id: Option<&str>, peer: Option<IpAddr>) -> bool {
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
