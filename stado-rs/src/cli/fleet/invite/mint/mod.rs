//! Minting: the randomness an invite's identity and credential come from, the
//! inputs the command parses, the one line an online invite is sent as, and
//! the removal of a channel key a mint could not finish using.

use base64::Engine;
use chrono::Duration;

use crate::cli::fleet::invite::{ID_BYTES, SECRET_BYTES};

pub(in crate::cli::fleet::invite) mod command;
mod offline;
mod preflight;

/// Random bytes from the operating system's CSPRNG. Time is not an ingredient:
/// anything derived from a clock is guessable by whoever knows roughly when it
/// was minted.
fn random_bytes(into: &mut [u8]) -> Result<(), String> {
    use ring::rand::SecureRandom;
    ring::rand::SystemRandom::new()
        .fill(into)
        .map_err(|_| "system randomness is unavailable".to_string())
}

/// A fresh public invite id. Minted on its own because an offline invite needs
/// an identity and must not mint a secret it would then have to be trusted to
/// throw away.
fn mint_id() -> Result<String, String> {
    let mut id_bytes = [0u8; ID_BYTES];
    random_bytes(&mut id_bytes)?;
    Ok(hex::encode(id_bytes))
}

/// A fresh invite secret — the credential of the online mode, and the only
/// thing in this module that must never be stored.
fn mint_secret() -> Result<String, String> {
    let mut secret_bytes = [0u8; SECRET_BYTES];
    random_bytes(&mut secret_bytes)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret_bytes))
}

/// Parse a duration like `30m`, `24h`, `7d`. A bare number is refused: the
/// unit of an invite's lifetime is exactly the kind of thing two people guess
/// differently.
pub fn parse_expiry(value: &str) -> Result<Duration, String> {
    let raw = value.trim();
    let (digits, unit) = raw.split_at(
        raw.find(|character: char| !character.is_ascii_digit())
            .ok_or_else(|| format!("--expires '{raw}': needs a unit, one of s, m, h, d"))?,
    );
    let amount: i64 = digits
        .parse()
        .map_err(|_| format!("--expires '{raw}': must be a number followed by s, m, h or d"))?;
    if amount <= 0 {
        return Err(format!("--expires '{raw}': must be positive"));
    }
    let span = match unit {
        "s" => Duration::try_seconds(amount),
        "m" => Duration::try_minutes(amount),
        "h" => Duration::try_hours(amount),
        "d" => Duration::try_days(amount),
        other => {
            return Err(format!(
                "--expires '{raw}': unknown unit '{other}', use s, m, h or d"
            ))
        }
    };
    span.ok_or_else(|| format!("--expires '{raw}': lifetime is out of range"))
}

/// Target name for an invite the operator did not name: derived from the
/// invite's own id, so the machine that shows up is traceable to the line that
/// invited it.
pub fn derived_target_name(id: &str) -> String {
    format!("invited-{}", &id[..8])
}

/// The one line the machine's owner runs. `/join.sh` needs no query
/// parameters: the script reveals nothing, and the secret arrives as its
/// argument.
pub fn join_command(api_url: &str, token: &str) -> String {
    format!("curl -fsSL {api_url}/join.sh | sh -s -- {token}")
}

/// Remove the credential item a failed mint left behind. Best effort by
/// necessity: the alternative to a failed delete is an unusable key staying in
/// the vault, and the caller is already returning an error naming the target it
/// belongs to.
async fn discard_minted_key(target_name: &str) {
    if let Ok(client) = crate::cli::fleet::key::configured_client() {
        let _ = client
            .delete_item(&crate::cli::fleet::key::item_id(target_name))
            .await;
    }
}
