//! Invite-based enrollment: `stado fleet invite|invites|revoke-invite`.
//!
//! The method exists because the previous shortest path to adding somebody
//! else's laptop was a phone call: the fleet reaches a machine over SSH with
//! the key it owns, so that key's public half had to be in the machine's
//! `authorized_keys` before `fleet enroll` could probe anything — and putting
//! it there was a human copy-paste on the far end. An invite moves that step
//! into a single line the machine's owner runs.
//!
//! What travels is a token, `<id>.<secret>`, and nothing else. The store keeps
//! only `secret_sha256`, so the object an operator (or a leak) can read cannot
//! be replayed as a credential; the secret exists in this process for exactly
//! as long as it takes to print it once, and no command can reprint it. The
//! key direction is unchanged and not negotiable: minting an invite mints the
//! fleet's own ed25519 pair through the existing `fleet key generate` path, the
//! private half stays in the operator's vault, and the machine only ever
//! receives the public half.
//!
//! Redemption is two dashboard routes authenticated by the token alone
//! (`GET /api/fleet/invite/key`, `POST /api/fleet/join`); neither may write the
//! registry. They spend the invite through [`authorize`] and [`spend`] here, so
//! the lifecycle has one implementation regardless of which surface drives it.

use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::queue::JobStorage;

/// Store prefix every invite object lives under. It sits beneath the join
/// requests' own prefix, so `fleet pending` (which lists `enrollments/`) must
/// keep ignoring documents it cannot parse as a request — it already does.
pub const INVITES_PREFIX: &str = "enrollments/invites/";

/// Invite lifecycle markers. `open` and `revoked` are stored; `spent` is both
/// stored and derived from the use counter; `expired` is only ever derived, so
/// an invite going stale needs no writer to notice it.
pub const STATUS_OPEN: &str = "open";
pub const STATUS_SPENT: &str = "spent";
pub const STATUS_REVOKED: &str = "revoked";
pub const STATUS_EXPIRED: &str = "expired";

/// Bytes of invite identity and of invite secret. The id is public and only
/// has to be unique; the secret is the credential.
const ID_BYTES: usize = 8;
const SECRET_BYTES: usize = 32;

/// One refusal for every unusable token. A caller learning *why* a token was
/// refused learns whether an id exists, whether it was already used and when
/// it lapsed — three answers an unauthenticated redeemer has no business
/// getting.
const REFUSED: &str = "invite token is not usable";

/// A stored invite. `secret_sha256` is the only trace of the secret anywhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    pub id: String,
    pub secret_sha256: String,
    pub target_name: String,
    pub created_at: String,
    pub expires_at: String,
    pub uses_allowed: u64,
    pub uses_spent: u64,
    pub status: String,
    pub created_by: String,
}

/// Store path of one invite.
pub fn invite_path(id: &str) -> String {
    format!("{INVITES_PREFIX}{id}.json")
}

/// Hex SHA-256 of a secret — the only form the store ever sees.
pub fn secret_digest(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    hex::encode(hasher.finalize())
}

fn is_invite_id(value: &str) -> bool {
    value.len() == ID_BYTES * 2
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Split a presented token into its public id and its secret. Pure, and
/// deliberately shape-only: a malformed token is refused with the same
/// sentence as a valid-looking one that does not exist.
pub fn parse_token(token: &str) -> Result<(&str, &str), String> {
    let (id, secret) = token.trim().split_once('.').ok_or_else(|| REFUSED.to_string())?;
    if !is_invite_id(id) || secret.is_empty() {
        return Err(REFUSED.to_string());
    }
    Ok((id, secret))
}

/// Mint a fresh `(id, secret)` pair from the operating system's CSPRNG. Time
/// is not an ingredient: a token derived from a clock is guessable by anyone
/// who knows roughly when it was minted.
fn mint_token() -> Result<(String, String), String> {
    use ring::rand::SecureRandom;
    let rng = ring::rand::SystemRandom::new();
    let mut id_bytes = [0u8; ID_BYTES];
    let mut secret_bytes = [0u8; SECRET_BYTES];
    rng.fill(&mut id_bytes)
        .map_err(|_| "system randomness is unavailable".to_string())?;
    rng.fill(&mut secret_bytes)
        .map_err(|_| "system randomness is unavailable".to_string())?;
    Ok((
        hex::encode(id_bytes),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret_bytes),
    ))
}

/// Parse a stored invite document. Pure.
pub fn parse_invite(document: &Value) -> Result<Invite, String> {
    let field = |name: &str| -> Result<String, String> {
        document
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("invite object has no '{name}'"))
    };
    let counter = |name: &str, fallback: u64| -> u64 {
        document.get(name).and_then(Value::as_u64).unwrap_or(fallback)
    };
    Ok(Invite {
        id: field("id")?,
        secret_sha256: field("secret_sha256")?,
        target_name: field("target_name")?,
        created_at: field("created_at")?,
        expires_at: field("expires_at")?,
        uses_allowed: counter("uses_allowed", 1),
        uses_spent: counter("uses_spent", 0),
        status: field("status")?,
        created_by: document
            .get("created_by")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

/// Render an invite as its stored document. Pure; carries no secret.
pub fn invite_document(invite: &Invite) -> Value {
    json!({
        "id": invite.id,
        "secret_sha256": invite.secret_sha256,
        "target_name": invite.target_name,
        "created_at": invite.created_at,
        "expires_at": invite.expires_at,
        "uses_allowed": invite.uses_allowed,
        "uses_spent": invite.uses_spent,
        "status": invite.status,
        "created_by": invite.created_by,
    })
}

/// The status an invite actually has now, which is not always the status on
/// disk: a revocation and an exhausted counter are recorded, a lapsed deadline
/// is not. Ordering is by permanence — revoked, then spent, then expired —
/// because a token that was used and then lapsed is more usefully reported as
/// spent. Pure.
pub fn effective_status(invite: &Invite, now: DateTime<Utc>) -> &'static str {
    if invite.status == STATUS_REVOKED {
        return STATUS_REVOKED;
    }
    if invite.status == STATUS_SPENT || invite.uses_spent >= invite.uses_allowed {
        return STATUS_SPENT;
    }
    match DateTime::parse_from_rfc3339(&invite.expires_at) {
        Ok(deadline) if now >= deadline.with_timezone(&Utc) => STATUS_EXPIRED,
        // An unparsable deadline is not a permit. Treating it as "never
        // expires" would turn one corrupt field into an eternal credential.
        Err(_) => STATUS_EXPIRED,
        Ok(_) => STATUS_OPEN,
    }
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
        other => return Err(format!("--expires '{raw}': unknown unit '{other}', use s, m, h or d")),
    };
    span.ok_or_else(|| format!("--expires '{raw}': lifetime is out of range"))
}

/// Target name for an invite the operator did not name: derived from the
/// invite's own id, so the machine that shows up is traceable to the line that
/// invited it.
pub fn derived_target_name(id: &str) -> String {
    format!("invited-{}", &id[..8])
}

async fn load_invite(store: &JobStorage, id: &str) -> Result<Option<Invite>, String> {
    let Some(text) = store
        .download_text(&invite_path(id))
        .await
        .map_err(|exc| exc.to_string())?
    else {
        return Ok(None);
    };
    let document: Value = serde_json::from_str(&text).map_err(|exc| exc.to_string())?;
    parse_invite(&document).map(Some)
}

async fn store_invite(store: &JobStorage, invite: &Invite) -> Result<(), String> {
    store
        .upload_text(
            &invite_path(&invite.id),
            &serde_json::to_string_pretty(&invite_document(invite)).map_err(|exc| exc.to_string())?,
        )
        .await
        .map_err(|exc| exc.to_string())
}

/// Every invite in the store, newest first, each with its effective status.
pub async fn list_invites(store: &JobStorage) -> Result<Vec<(Invite, &'static str)>, String> {
    let blobs = store
        .list_blobs_with_meta(INVITES_PREFIX)
        .await
        .map_err(|exc| exc.to_string())?;
    let now = Utc::now();
    let mut found = Vec::new();
    for blob in &blobs {
        let Some(text) = store
            .download_text(&blob.name)
            .await
            .map_err(|exc| exc.to_string())?
        else {
            continue;
        };
        let Ok(document) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let Ok(invite) = parse_invite(&document) else {
            continue;
        };
        let status = effective_status(&invite, now);
        found.push((invite, status));
    }
    found.sort_by(|left, right| right.0.created_at.cmp(&left.0.created_at));
    Ok(found)
}

/// Compare two digests without leaking where they first differ. A comparison
/// that returns on the first differing byte is a byte-at-a-time oracle for the
/// stored digest, and the stored digest is what a redeemer must be able to
/// produce. Same technique the dashboard's bearer check uses: fold the whole
/// difference, decide once.
pub fn digests_match(stored: &str, presented: &str) -> bool {
    let (Ok(stored), Ok(presented)) = (hex::decode(stored), hex::decode(presented)) else {
        return false;
    };
    if stored.len() != presented.len() {
        return false;
    }
    let mut difference = u8::default();
    for (left, right) in stored.iter().zip(&presented) {
        difference |= left ^ right;
    }
    difference == u8::default()
}

/// Resolve a presented token to the invite it may spend, or refuse without
/// saying which of unknown/spent/revoked/expired applies.
pub async fn authorize(store: &JobStorage, token: &str) -> Result<Invite, String> {
    let (id, secret) = parse_token(token)?;
    let invite = load_invite(store, id).await?.ok_or(REFUSED)?;
    if !digests_match(&invite.secret_sha256, &secret_digest(secret)) {
        return Err(REFUSED.to_string());
    }
    if effective_status(&invite, Utc::now()) != STATUS_OPEN {
        return Err(REFUSED.to_string());
    }
    Ok(invite)
}

/// Count one redemption against an invite, closing it when the allowance runs
/// out. Called by the redemption route after the request it authorized has been
/// filed.
pub async fn spend(store: &JobStorage, invite: &Invite) -> Result<Invite, String> {
    let mut spent = invite.clone();
    spent.uses_spent = spent.uses_spent.saturating_add(1);
    if spent.uses_spent >= spent.uses_allowed {
        spent.status = STATUS_SPENT.to_string();
    }
    store_invite(store, &spent).await?;
    Ok(spent)
}

/// Close an invite that has produced a registered target: approval is the end
/// of its life regardless of any allowance left over.
pub async fn mark_spent(store: &JobStorage, id: &str) -> Result<(), String> {
    let Some(mut invite) = load_invite(store, id).await? else {
        return Ok(());
    };
    if invite.status == STATUS_SPENT {
        return Ok(());
    }
    invite.status = STATUS_SPENT.to_string();
    store_invite(store, &invite).await
}

/// The one line the machine's owner runs. `/join.sh` needs no query
/// parameters: the script reveals nothing, and the secret arrives as its
/// argument.
pub fn join_command(api_url: &str, token: &str) -> String {
    format!("curl -fsSL {api_url}/join.sh | sh -s -- {token}")
}

/// Refuse a target name already taken by a registered machine or by a live
/// invite. Silently suffixing a colliding name is how two machines end up
/// sharing one channel key. Pure.
pub fn preflight_invite_name(
    document: &Value,
    live: &[(Invite, &'static str)],
    name: &str,
) -> Result<(), String> {
    let targets = document
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| "registry.targets: must be an array".to_string())?;
    if targets
        .iter()
        .any(|target| target.get("name").and_then(Value::as_str) == Some(name))
    {
        return Err(format!(
            "target '{name}' is already registered; invite a different name with --name"
        ));
    }
    if let Some((invite, _)) = live
        .iter()
        .find(|(invite, status)| *status == STATUS_OPEN && invite.target_name == name)
    {
        return Err(format!(
            "invite {} is already open for target '{name}'; revoke it or use --name",
            invite.id
        ));
    }
    Ok(())
}

/// `stado fleet invite [--name NAME] [--expires 24h] [--uses 1]` — mint the
/// channel key for a machine nobody has touched yet, plus the one line its
/// owner runs.
///
/// The order is deliberate: the key is minted first, because an invite whose
/// key does not exist is a token that fails at redemption on somebody else's
/// laptop. If recording the invite then fails, the freshly minted credential
/// item is removed again — a half-minted invite leaves nothing behind.
pub async fn invite(
    name: Option<&str>,
    expires: &str,
    uses: u64,
    as_json: bool,
) -> Result<bool, String> {
    if uses == 0 {
        return Err("--uses must be at least 1".to_string());
    }
    let lifetime = parse_expiry(expires)?;
    let document = crate::cli::registry::fetch_document()
        .await
        .map_err(|exc| exc.to_string())?;
    crate::cli::fleet::enroll::catalog::require_invite_allowed(&document)?;
    let store = JobStorage::new().await.map_err(|exc| exc.to_string())?;
    let live = list_invites(&store).await?;
    let (id, secret) = mint_token()?;
    let target_name = match name {
        Some(given) => given.to_string(),
        None => derived_target_name(&id),
    };
    preflight_invite_name(&document, &live, &target_name)?;

    let runner = crate::deploy::production_runner();
    let (public_key, fingerprint) =
        crate::cli::fleet::key::rotate::generate_stored(&runner, &target_name).await?;

    let created_at = Utc::now();
    let invite = Invite {
        id: id.clone(),
        secret_sha256: secret_digest(&secret),
        target_name: target_name.clone(),
        created_at: created_at.to_rfc3339(),
        expires_at: (created_at + lifetime).to_rfc3339(),
        uses_allowed: uses,
        uses_spent: 0,
        status: STATUS_OPEN.to_string(),
        created_by: crate::providers::vast::system_hostname(),
    };
    let recorded = store
        .create_text_if_absent(
            &invite_path(&id),
            &serde_json::to_string_pretty(&invite_document(&invite)).map_err(|exc| exc.to_string())?,
        )
        .await;
    match recorded {
        Ok(true) => {}
        Ok(false) | Err(_) => {
            let detail = match recorded {
                Err(exc) => exc.to_string(),
                _ => format!("invite id {id} already exists in the store"),
            };
            if let Ok(client) = crate::cli::fleet::key::configured_client() {
                let _ = client
                    .delete_item(&crate::cli::fleet::key::item_id(&target_name))
                    .await;
            }
            return Err(format!(
                "could not record the invite ({detail}); the minted key for '{target_name}' was removed"
            ));
        }
    }

    let api_url = crate::config::stado_api_url();
    let token = format!("{id}.{secret}");
    let command = if api_url.is_empty() {
        None
    } else {
        Some(join_command(&api_url, &token))
    };
    let line = crate::cli::fleet::key::authorized_keys_line(
        &public_key,
        &crate::cli::fleet::key::item_id(&target_name),
    );
    if as_json {
        let rendered = json!({
            "id": invite.id,
            "token": token,
            "target_name": invite.target_name,
            "expires_at": invite.expires_at,
            "uses_allowed": invite.uses_allowed,
            "join_command": command,
            "public_key": public_key,
            "authorized_keys_line": line,
            "token_shown_once": true,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&rendered).map_err(|exc| exc.to_string())?
        );
        return Ok(true);
    }
    println!("invite {} for target '{}'", invite.id, invite.target_name);
    println!("token: {token}");
    println!("  this is the only time the token is shown; nothing can reprint it");
    println!("expires: {} (uses allowed: {uses})", invite.expires_at);
    println!("channel key minted: {fingerprint}");
    match command {
        Some(command) => {
            println!("send this one line to the machine's owner:");
            println!("  {command}");
        }
        None => {
            println!(
                "STADO_API_URL is not configured here, so the one-liner cannot name the control plane;"
            );
            println!(
                "  set it and the line is: curl -fsSL <STADO_API_URL>/join.sh | sh -s -- <token>"
            );
        }
    }
    println!("then approve the machine: stado fleet pending, stado fleet approve <hostname>");
    Ok(true)
}

/// `stado fleet invites` — every invite and the state it is actually in.
pub async fn invites(as_json: bool) -> Result<bool, String> {
    let store = JobStorage::new().await.map_err(|exc| exc.to_string())?;
    let found = list_invites(&store).await?;
    if as_json {
        let rendered = json!({
            "invites": found
                .iter()
                .map(|(invite, status)| json!({
                    "id": invite.id,
                    "target_name": invite.target_name,
                    "status": status,
                    "created_at": invite.created_at,
                    "expires_at": invite.expires_at,
                    "uses_allowed": invite.uses_allowed,
                    "uses_spent": invite.uses_spent,
                    "created_by": invite.created_by,
                }))
                .collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&rendered).map_err(|exc| exc.to_string())?
        );
        return Ok(true);
    }
    if found.is_empty() {
        println!("no invites");
        return Ok(true);
    }
    for (invite, status) in &found {
        println!(
            "{}\t{}\t{}\t{}/{}\texpires {}",
            invite.id,
            invite.target_name,
            status,
            invite.uses_spent,
            invite.uses_allowed,
            invite.expires_at
        );
    }
    Ok(true)
}

/// `stado fleet revoke-invite ID` — retire an invite before anybody uses it.
pub async fn revoke_invite(id: &str) -> Result<bool, String> {
    let store = JobStorage::new().await.map_err(|exc| exc.to_string())?;
    let mut invite = load_invite(&store, id)
        .await?
        .ok_or_else(|| format!("no invite '{id}'"))?;
    if invite.status == STATUS_REVOKED {
        println!("invite {id} is already revoked");
        return Ok(true);
    }
    let previous = effective_status(&invite, Utc::now());
    invite.status = STATUS_REVOKED.to_string();
    store_invite(&store, &invite).await?;
    println!("invite {id} for target '{}' is revoked (was {previous})", invite.target_name);
    println!(
        "the minted channel key is still in the credential store: stado fleet key rm '{}' removes it",
        invite.target_name
    );
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invite_at(expires_at: &str, uses_allowed: u64, uses_spent: u64, status: &str) -> Invite {
        Invite {
            id: "0123456789abcdef".to_string(),
            secret_sha256: secret_digest("s"),
            target_name: "studio".to_string(),
            created_at: "2026-01-01T00:00:00+00:00".to_string(),
            expires_at: expires_at.to_string(),
            uses_allowed,
            uses_spent,
            status: status.to_string(),
            created_by: "mini".to_string(),
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-01-02T00:00:00+00:00")
            .expect("fixed instant")
            .with_timezone(&Utc)
    }

    #[test]
    fn minted_tokens_are_unique_and_shaped_as_declared() {
        let (first_id, first_secret) = mint_token().expect("mint");
        let (second_id, second_secret) = mint_token().expect("mint");
        assert!(is_invite_id(&first_id), "id shape: {first_id}");
        assert_ne!(first_id, second_id);
        assert_ne!(first_secret, second_secret);
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&first_secret)
            .expect("base64url without padding");
        assert_eq!(decoded.len(), SECRET_BYTES);
        assert!(!first_secret.contains('='));
        let (id, secret) = parse_token(&format!("{first_id}.{first_secret}")).expect("parse");
        assert_eq!((id, secret), (first_id.as_str(), first_secret.as_str()));
    }

    #[test]
    fn digest_comparison_accepts_only_the_matching_secret() {
        let stored = secret_digest("the-secret");
        assert!(digests_match(&stored, &secret_digest("the-secret")));
        assert!(!digests_match(&stored, &secret_digest("the-secreu")));
        // A digest whose first bytes match must not pass on the strength of a
        // prefix, and non-hex or short input is a refusal, not a panic.
        assert!(!digests_match(&stored, &stored[..62]));
        assert!(!digests_match(&stored, "zz"));
        assert!(!digests_match(&stored, ""));
    }

    #[test]
    fn malformed_tokens_are_refused_without_detail() {
        for candidate in ["", "nodot", "short.secret", "0123456789abcdef.", "01234.56789abcdefg.s"] {
            let error = parse_token(candidate).unwrap_err();
            assert_eq!(error, REFUSED, "leaked detail for {candidate:?}");
        }
    }

    #[test]
    fn stored_document_carries_the_digest_and_never_the_secret() {
        let invite = invite_at("2026-01-03T00:00:00+00:00", 1, 0, STATUS_OPEN);
        let document = invite_document(&invite);
        let text = document.to_string();
        assert!(text.contains("secret_sha256"));
        assert!(!text.contains("\"secret\""));
        assert_eq!(parse_invite(&document).expect("round trip"), invite);
    }

    #[test]
    fn status_is_derived_for_expiry_and_recorded_for_the_rest() {
        assert_eq!(
            effective_status(&invite_at("2026-01-03T00:00:00+00:00", 1, 0, STATUS_OPEN), now()),
            STATUS_OPEN
        );
        assert_eq!(
            effective_status(&invite_at("2026-01-01T12:00:00+00:00", 1, 0, STATUS_OPEN), now()),
            STATUS_EXPIRED
        );
        assert_eq!(
            effective_status(&invite_at("2026-01-03T00:00:00+00:00", 1, 1, STATUS_OPEN), now()),
            STATUS_SPENT
        );
        assert_eq!(
            effective_status(&invite_at("2026-01-03T00:00:00+00:00", 3, 0, STATUS_REVOKED), now()),
            STATUS_REVOKED
        );
        assert_eq!(
            effective_status(&invite_at("not a timestamp", 1, 0, STATUS_OPEN), now()),
            STATUS_EXPIRED
        );
    }

    #[test]
    fn expiry_needs_a_unit_and_a_positive_amount() {
        assert_eq!(parse_expiry("24h").expect("hours"), Duration::hours(24));
        assert_eq!(parse_expiry("7d").expect("days"), Duration::days(7));
        assert_eq!(parse_expiry("90m").expect("minutes"), Duration::minutes(90));
        for bad in ["", "24", "0h", "-1h", "24w", "h"] {
            assert!(parse_expiry(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn colliding_target_names_are_refused_not_suffixed() {
        let document = serde_json::json!({ "targets": [{ "name": "studio" }] });
        let error = preflight_invite_name(&document, &[], "studio").unwrap_err();
        assert!(error.contains("already registered"), "unexpected error: {error}");
        let empty = serde_json::json!({ "targets": [] });
        let open = vec![(
            invite_at("2026-01-03T00:00:00+00:00", 1, 0, STATUS_OPEN),
            STATUS_OPEN,
        )];
        let error = preflight_invite_name(&empty, &open, "studio").unwrap_err();
        assert!(error.contains("already open"), "unexpected error: {error}");
        let closed = vec![(
            invite_at("2026-01-03T00:00:00+00:00", 1, 1, STATUS_SPENT),
            STATUS_SPENT,
        )];
        preflight_invite_name(&empty, &closed, "studio").expect("a spent invite frees the name");
    }

    #[test]
    fn derived_name_follows_the_invite_id() {
        assert_eq!(derived_target_name("0123456789abcdef"), "invited-01234567");
    }

    #[test]
    fn join_command_carries_the_token_as_an_argument() {
        assert_eq!(
            join_command("https://stado.wisent.com", "abc.def"),
            "curl -fsSL https://stado.wisent.com/join.sh | sh -s -- abc.def"
        );
    }
}
