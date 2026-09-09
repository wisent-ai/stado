//! The Brama sign-in journal a case writes, at the instants it chooses.
//!
//! One `subscription_sign_in` record per attempt, carrying the login item, the
//! instant, the verdict and the trajectory's own tail — which is where the
//! Google SSO driver's markers and Google's own sentences land, and what the
//! host-side reader matches its fixed marker vocabulary against. Nothing here
//! reduces an attempt: the product's reader does that on the host, which is
//! the half these cases exist to exercise.

use chrono::{SecondsFormat, Utc};

/// Trajectory tails copied from what the Google SSO driver and Google itself
/// wrote on charless-mac-mini. Each one is the text behind exactly one marker
/// name in the host reader's vocabulary.
pub const CODE_ACCEPTED: &str = "[google_sso] filled Google Authenticator TOTP code; signed in";
pub const CODE_REFUSED: &str =
    "[google_sso] filled Google Authenticator TOTP code; Google said: Wrong code. Try again.";
pub const LOCKED_OUT: &str = "[google_sso] Too many failed attempts. Try again later.";
pub const RUNTIME_BROKEN: &str =
    "ERR_MODULE_NOT_FOUND: Cannot find module '/Users/x/dist/worker/dispatch.js'";

/// The two verdicts a recorded attempt carries.
pub const SIGNED_IN: &str = "signed_in";
pub const FAILED: &str = "failed";

/// One recorded sign-in attempt, dated once at construction so a case can
/// assert the product's answer against the exact instant it wrote.
pub struct Attempt {
    item: String,
    result: &'static str,
    detail: String,
    /// The instant in the journal's own spelling.
    pub at: String,
}

/// An attempt against `item` that happened `seconds_ago`, whose trajectory
/// tail is `detail`.
pub fn attempt(item: &str, seconds_ago: i64, result: &'static str, detail: &str) -> Attempt {
    let stamp = Utc::now() - chrono::Duration::seconds(seconds_ago);
    Attempt {
        item: item.to_string(),
        result,
        detail: detail.to_string(),
        at: stamp.to_rfc3339_opts(SecondsFormat::Secs, true),
    }
}

/// Write these attempts as the host's journal, in the order given.
pub fn write(path: &std::path::Path, attempts: &[Attempt]) {
    let mut text = String::new();
    for entry in attempts {
        let record = serde_json::json!({
            "kind": "subscription_sign_in",
            "login_item": entry.item,
            "provider": "google",
            "at": entry.at,
            "result": entry.result,
            "detail": entry.detail,
        });
        text.push_str(&serde_json::to_string(&record).expect("the record serialises"));
        text.push('\n');
    }
    std::fs::write(path, text).expect("write the host's sign-in journal");
}
