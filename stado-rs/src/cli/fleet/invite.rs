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
//!
//! That is the ONLINE mode, and it needs one thing the fleet does not always
//! have: a control point the machine's owner can reach over HTTP. When there is
//! none — the name does not resolve, nothing listens, or the release serving it
//! predates the invite routes — printing the one-liner anyway would hand
//! somebody a command that cannot work, so [`invite`] probes `/join.sh` first
//! and falls back to the OFFLINE mode instead of lying.
//!
//! The offline mode carries no secret and uses no route. What travels is a
//! self-contained `sh` fragment, over whatever channel the operator is already
//! using to talk to the machine's owner, and the only key in it is the fleet's
//! PUBLIC half: intercepting the fragment gains nothing. The owner runs it, the
//! fragment installs the key and prints the `user@address` to send back, and the
//! operator closes the invite with the ordinary
//! `fleet enroll NAME --ssh ADDRESS --bootstrap` — which reaches
//! [`close_offline_for_target`] and spends the invite through the same
//! [`mark_spent`] that `approve` uses. No second state machine.

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

/// How an invite is redeemed. `online` is the token-and-route mode; `offline`
/// is the pasted-fragment mode, which has no secret to present and no route to
/// present it to. An object written before the modes existed has no `mode`
/// field and is read as `online`, which is what it was.
pub const MODE_ONLINE: &str = "online";
pub const MODE_OFFLINE: &str = "offline";

/// Bytes of invite identity and of invite secret. The id is public and only
/// has to be unique; the secret is the credential.
const ID_BYTES: usize = 8;
const SECRET_BYTES: usize = 32;

/// One refusal for every unusable token. A caller learning *why* a token was
/// refused learns whether an id exists, whether it was already used and when
/// it lapsed — three answers an unauthenticated redeemer has no business
/// getting.
const REFUSED: &str = "invite token is not usable";

/// A stored invite. `secret_sha256` is the only trace of the secret anywhere,
/// and it is empty for exactly one reason: an offline invite never had a
/// secret. An empty digest is not a weak digest — no input hashes to it, so a
/// presented token cannot match one, which is the same refusal an unknown id
/// gets.
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
    pub mode: String,
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
    let (id, secret) = token
        .trim()
        .split_once('.')
        .ok_or_else(|| REFUSED.to_string())?;
    if !is_invite_id(id) || secret.is_empty() {
        return Err(REFUSED.to_string());
    }
    Ok((id, secret))
}

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

/// Parse a stored invite document. Pure.
///
/// `secret_sha256` is required of an online invite and refused of an offline
/// one: a stored offline object carrying a digest would mean something minted
/// a secret for a mode that has nothing to present it to.
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
        document
            .get(name)
            .and_then(Value::as_u64)
            .unwrap_or(fallback)
    };
    let mode = match document.get("mode").and_then(Value::as_str) {
        None | Some("") | Some(MODE_ONLINE) => MODE_ONLINE.to_string(),
        Some(MODE_OFFLINE) => MODE_OFFLINE.to_string(),
        Some(other) => return Err(format!("invite object has an unknown mode '{other}'")),
    };
    let secret_sha256 = if mode == MODE_OFFLINE {
        if document.get("secret_sha256").is_some() {
            return Err("an offline invite object must carry no 'secret_sha256'".to_string());
        }
        String::new()
    } else {
        field("secret_sha256")?
    };
    Ok(Invite {
        id: field("id")?,
        secret_sha256,
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
        mode,
    })
}

/// Render an invite as its stored document. Pure; carries no secret. An
/// offline invite has no digest to write, so the key is absent rather than
/// present and empty — the store never holds a field that reads like a
/// credential nobody can use.
pub fn invite_document(invite: &Invite) -> Value {
    let mut document = json!({
        "id": invite.id,
        "target_name": invite.target_name,
        "created_at": invite.created_at,
        "expires_at": invite.expires_at,
        "uses_allowed": invite.uses_allowed,
        "uses_spent": invite.uses_spent,
        "status": invite.status,
        "created_by": invite.created_by,
        "mode": invite.mode,
    });
    if !invite.secret_sha256.is_empty() {
        document["secret_sha256"] = Value::String(invite.secret_sha256.clone());
    }
    document
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
            &serde_json::to_string_pretty(&invite_document(invite))
                .map_err(|exc| exc.to_string())?,
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
///
/// An offline invite is refused here by mode as well as by digest. Its stored
/// digest is empty, so no token could match it anyway; saying so explicitly
/// means a future writer that puts a digest on an offline object still cannot
/// turn a pasted fragment into a redeemable credential.
pub async fn authorize(store: &JobStorage, token: &str) -> Result<Invite, String> {
    let (id, secret) = parse_token(token)?;
    let invite = load_invite(store, id).await?.ok_or(REFUSED)?;
    if invite.mode == MODE_OFFLINE {
        return Err(REFUSED.to_string());
    }
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

/// How long a control-point probe may take. An invite is minted while somebody
/// waits for the answer, and a checkpoint slower than this is not one the
/// machine's owner can use either.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Machine-readable verdicts of [`probe_checkpoint`]. The three refusals an
/// operator fixes by different means are named separately on purpose: a name
/// with no DNS answer needs a record, a refused connection needs a listener or
/// a tunnel, and a live server that does not know the route needs a newer
/// release.
pub const REASON_OK: &str = "ok";
pub const REASON_UNRESOLVED: &str = "name_does_not_resolve";
pub const REASON_CONNECTION_REFUSED: &str = "connection_refused";
pub const REASON_ROUTE_UNKNOWN: &str = "route_unknown";
pub const REASON_NOT_CONFIGURED: &str = "not_configured";
pub const REASON_FORCED_OFFLINE: &str = "forced_offline";

/// What `invite` found out about the control point before deciding which mode
/// it can honestly offer. `reason` is the verdict a program branches on,
/// `detail` the sentence a human reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub url: String,
    pub probed: bool,
    pub reachable: bool,
    pub reason: &'static str,
    pub detail: String,
}

impl Checkpoint {
    fn refused(url: &str, reason: &'static str, detail: String) -> Self {
        Self {
            url: url.to_string(),
            probed: true,
            reachable: false,
            reason,
            detail,
        }
    }

    /// The mode an invite can be issued in given this verdict. Online is
    /// reachable-only: everything else, including a control point nobody
    /// configured, is offline.
    pub fn mode(&self) -> &'static str {
        if self.reachable {
            MODE_ONLINE
        } else {
            MODE_OFFLINE
        }
    }
}

/// The verdict as a document. Pure.
pub fn checkpoint_document(checkpoint: &Checkpoint) -> Value {
    json!({
        "url": checkpoint.url,
        "probed": checkpoint.probed,
        "reachable": checkpoint.reachable,
        "reason": checkpoint.reason,
        "detail": checkpoint.detail,
    })
}

/// Host and port `/join.sh` would be fetched from. Pure.
pub fn probe_authority(base: &str) -> Result<(String, u16), String> {
    let parsed = url::Url::parse(base).map_err(|exc| exc.to_string())?;
    let host = parsed
        .host_str()
        .filter(|host| !host.is_empty())
        .ok_or_else(|| "the address names no host".to_string())?;
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| format!("scheme '{}' has no port", parsed.scheme()))?;
    Ok((host.to_string(), port))
}

/// Where the base address came from. The one-liner's usefulness depends on
/// this: an address from configuration is as durable as the deployment behind
/// it, and an address from a quick tunnel lasts exactly as long as that tunnel
/// does — which is a sentence the operator has to read, because the person who
/// runs the one-liner reads nothing at all.
pub const BASE_FROM_ENROLLMENT_URL: &str = "enrollment.url";
pub const BASE_FROM_INGRESS: &str = "ingress";
pub const BASE_FROM_API_URL: &str = "api.url";

/// Origin the invite probe and the printed one-liner are built from:
/// `enrollment.url` when configured, else `api.url`.
///
/// A deployment that publishes only the narrow `stado dashboard
/// --enrollment-only` listener has an enrollment origin that is not its
/// deployment endpoint, and the owner of a new machine can only reach the
/// former. Falling back to `api.url` keeps every existing deployment — which
/// serves enrollment from the same origin as everything else — unchanged.
pub fn enrollment_base() -> String {
    let enrollment = crate::config::enrollment_url();
    if enrollment.is_empty() {
        crate::config::stado_api_url()
    } else {
        enrollment
    }
}

/// The base an online invite is built on, in the order that puts the most
/// deliberate answer first.
///
/// 1. `enrollment.url`. Somebody configured an enrollment origin; nothing this
///    process discovers may override a decision that was written down.
/// 2. The published `enrollments/ingress.json`, **if its address still
///    answers**. This is the entrance `stado fleet ingress up` stood up, and it
///    is the whole reason the one-line mode is reachable on a fleet with no
///    public deployment. It is used only when it is live: a stale object from a
///    tunnel that has since closed must not become a one-liner, which is the
///    same rule the probe has always enforced, applied one step earlier.
/// 3. `api.url`, the deployment endpoint — unchanged, and still what every
///    fleet that serves enrollment from its own origin gets.
///
/// Returns the base and which of the three it is, so the caller can say out
/// loud that a tunnel address is temporary.
pub async fn resolve_invite_base(store: &JobStorage) -> (String, &'static str) {
    let configured = crate::config::enrollment_url();
    if !configured.is_empty() {
        return (configured, BASE_FROM_ENROLLMENT_URL);
    }
    if let Ok(Some(ingress)) = crate::cli::fleet::ingress::published(store).await {
        if probe_checkpoint(&ingress.base_url).await.reachable {
            return (ingress.base_url, BASE_FROM_INGRESS);
        }
    }
    (crate::config::stado_api_url(), BASE_FROM_API_URL)
}

/// Ask the configured control point for `/join.sh` before anybody is told to
/// fetch it.
///
/// Resolution is asked for on its own, ahead of the request, so a name with no
/// DNS answer is not reported as a refused connection — the client would
/// collapse both into one transport error, and they are not the same problem.
/// Only a 200 counts: a live server answering 404 knows nothing about invites,
/// which is a release older than these routes, not a network fault.
pub async fn probe_checkpoint(base: &str) -> Checkpoint {
    if base.is_empty() {
        return Checkpoint {
            url: String::new(),
            probed: false,
            reachable: false,
            reason: REASON_NOT_CONFIGURED,
            detail: "no control point is configured (STADO_ENROLLMENT_URL / stado config \
                     enrollment.url and STADO_API_URL / stado config api.url are both empty)"
                .to_string(),
        };
    }
    let endpoint = format!("{base}/join.sh");
    let (host, port) = match probe_authority(base) {
        Ok(authority) => authority,
        Err(detail) => {
            return Checkpoint::refused(
                base,
                REASON_UNRESOLVED,
                format!("control point '{base}' is not a usable address ({detail})"),
            );
        }
    };
    let resolved = tokio::task::spawn_blocking({
        let host = host.clone();
        move || {
            std::net::ToSocketAddrs::to_socket_addrs(&(host.as_str(), port))
                .map(|addresses| addresses.count())
                .unwrap_or_default()
        }
    })
    .await
    .unwrap_or_default();
    if resolved == 0 {
        return Checkpoint::refused(
            base,
            REASON_UNRESOLVED,
            format!(
                "control point '{host}' does not resolve to any address, so nothing can fetch /join.sh from it"
            ),
        );
    }
    let client = match reqwest::Client::builder().timeout(PROBE_TIMEOUT).build() {
        Ok(client) => client,
        Err(exc) => {
            return Checkpoint::refused(
                base,
                REASON_CONNECTION_REFUSED,
                format!("this host could not build an HTTP client to probe {endpoint} ({exc})"),
            );
        }
    };
    match client.get(&endpoint).send().await {
        Err(exc) => Checkpoint::refused(
            base,
            REASON_CONNECTION_REFUSED,
            format!(
                "nothing answered at {endpoint} (connection refused or timed out): {}",
                exc.without_url()
            ),
        ),
        Ok(response) if response.status().as_u16() == 200 => Checkpoint {
            url: base.to_string(),
            probed: true,
            reachable: true,
            reason: REASON_OK,
            detail: format!("{endpoint} answered 200"),
        },
        Ok(response) => Checkpoint::refused(
            base,
            REASON_ROUTE_UNKNOWN,
            format!(
                "{endpoint} answered HTTP {}, not 200: the release serving that host is older than the invite routes",
                response.status().as_u16()
            ),
        ),
    }
}

/// The offline fragment, verbatim, with `@TARGET@` and `@FLEET_KEY@` left to
/// substitute. One literal so that what an operator reads before sending it is
/// exactly what runs on the far machine.
///
/// It wraps itself in `sh <<'...'`: the fragment is pasted into whatever
/// interactive shell the owner already has open, and a body that runs under
/// `set -eu` and calls `exit` on a missing tool must not be able to close that
/// shell.
///
/// The address rules are `deploy/join.sh`'s, in its order — tailnet DNS name,
/// then a multicast `.local` name only where something answers for it, then the
/// IPv4 address of the default interface, then the bare hostname. Two commands
/// choosing an address by different rules would report two different machines.
const OFFLINE_SNIPPET: &str = r##"sh <<'STADO_OFFLINE_INVITE'
# stado offline invite for '@TARGET@' -- run this ON THE MACHINE BEING ADDED.
#
# Nothing in this text is a secret. The only key in it is the fleet's PUBLIC
# half; the private half never leaves the operator's vault, so whoever reads
# this fragment gains no access to anything, here or anywhere else.
#
# What it does, and nothing more:
#   1. creates ~/.ssh (mode 700) and ~/.ssh/authorized_keys (mode 600),
#   2. appends the fleet's public key there, once, even if it runs twice,
#   3. checks whether an SSH server answers on port 22 and, if not, prints how
#      to turn one on -- it never turns anything on itself,
#   4. prints the user@address to send back to the operator.
# It installs no software, starts no service, and generates no key.
set -eu

fleet_key='@FLEET_KEY@'
fleet_target='@TARGET@'

say() {
    printf '%s\n' "$*"
}

die() {
    printf '%s\n' "$*" >&2
    exit 1
}

for required in awk mkdir chmod tail uname id; do
    command -v "$required" >/dev/null 2>&1 ||
        die "this machine has no $required, which this fragment needs"
done

key_type="$(printf '%s' "$fleet_key" | awk '{ print $1 }')"
key_blob="$(printf '%s' "$fleet_key" | awk '{ print $2 }')"
[ -n "$key_blob" ] || die 'the pasted fragment carries no key material'

# ------------------------------------------------------------- install the key

ssh_dir="$HOME/.ssh"
authorized_keys="$ssh_dir/authorized_keys"

[ -d "$ssh_dir" ] || mkdir -p "$ssh_dir"
chmod 700 "$ssh_dir"
[ -f "$authorized_keys" ] || (umask 077; : >"$authorized_keys")
chmod 600 "$authorized_keys"

# Idempotent on the key material, not on the whole line: a re-issued invite may
# carry a different comment, and a second run must not leave the same key twice.
if awk -v type="$key_type" -v blob="$key_blob" '
        $1 == type && $2 == blob { found = 1 }
        END { exit found ? 0 : 1 }
    ' "$authorized_keys"; then
    key_action='already present'
else
    # Append on its own line even when the file did not end in a newline.
    if [ -s "$authorized_keys" ] && [ -n "$(tail -c 1 "$authorized_keys")" ]; then
        printf '\n' >>"$authorized_keys"
    fi
    printf '%s\n' "$fleet_key" >>"$authorized_keys"
    key_action='installed'
fi

# ------------------------------------------------------------- reachability

os_name="$(uname -s)"
login_user="$(id -un)"
short_hostname="$(uname -n | awk '{ sub(/\..*$/, "", $0); print tolower($0) }')"
[ -n "$short_hostname" ] || die 'this machine does not report a hostname'

tailscale_bin=''
if command -v tailscale >/dev/null 2>&1; then
    tailscale_bin="$(command -v tailscale)"
else
    for candidate in \
        /Applications/Tailscale.app/Contents/MacOS/Tailscale \
        /usr/local/bin/tailscale \
        /opt/homebrew/bin/tailscale
    do
        if [ -x "$candidate" ]; then
            tailscale_bin="$candidate"
            break
        fi
    done
fi

address=''
address_kind=''
if [ -n "$tailscale_bin" ]; then
    # --peers=false leaves exactly this machine's own record, so the DNSName
    # read back cannot be some other node's.
    tailnet_name="$("$tailscale_bin" status --json --peers=false 2>/dev/null |
        awk 'match($0, /"DNSName"[ \t]*:[ \t]*"[^"]*"/) {
                field = substr($0, RSTART, RLENGTH)
                sub(/^"DNSName"[ \t]*:[ \t]*"/, "", field)
                sub(/"$/, "", field)
                print field
                exit
            }' || true)"
    tailnet_name="${tailnet_name%.}"
    case "$tailnet_name" in
        ''|*[!A-Za-z0-9.-]*) ;;
        *)
            address="$tailnet_name"
            address_kind='tailnet name'
            ;;
    esac
fi

if [ -z "$address" ]; then
    case "$os_name" in
        Darwin)
            local_name="$(scutil --get LocalHostName 2>/dev/null || true)"
            if [ -n "$local_name" ]; then
                address="$local_name.local"
                address_kind='multicast DNS name'
            fi
            ;;
        Linux)
            # Only claim .local where something actually answers for it.
            if [ -S /run/avahi-daemon/socket ] || [ -S /var/run/avahi-daemon/socket ]; then
                address="$short_hostname.local"
                address_kind='multicast DNS name'
            fi
            ;;
    esac
fi

if [ -z "$address" ]; then
    case "$os_name" in
        Darwin)
            default_if="$(route -n get default 2>/dev/null |
                awk '$1 == "interface:" { print $2; exit }')"
            if [ -n "$default_if" ]; then
                address="$(ipconfig getifaddr "$default_if" 2>/dev/null || true)"
            fi
            ;;
        Linux)
            address="$(ip route get 1.1.1.1 2>/dev/null |
                awk '{ for (i = 1; i < NF; i++) if ($i == "src") { print $(i + 1); exit } }')"
            if [ -z "$address" ] && command -v hostname >/dev/null 2>&1; then
                address="$(hostname -I 2>/dev/null | awk '{ print $1 }')"
            fi
            ;;
    esac
    if [ -n "$address" ]; then
        address_kind='IPv4 address of the default interface'
    fi
fi

if [ -z "$address" ]; then
    address="$short_hostname"
    address_kind='bare hostname (nothing better was resolvable)'
fi

# ------------------------------------------------------------- sshd probe

# The fleet dials in over SSH, so a machine with no SSH server answering is
# reachable in name only. Remote Login is the owner's decision and needs
# administrator rights: diagnose it, print the exact way to turn it on, and
# never turn it on here.
ssh_listening='unknown'
if command -v nc >/dev/null 2>&1; then
    if nc -z -w 3 127.0.0.1 22 >/dev/null 2>&1; then
        ssh_listening='yes'
    else
        ssh_listening='no'
    fi
elif command -v ssh >/dev/null 2>&1; then
    ssh_probe="$(ssh -o BatchMode=yes -o StrictHostKeyChecking=no \
        -o UserKnownHostsFile=/dev/null -o ConnectTimeout=5 \
        127.0.0.1 true 2>&1 || true)"
    case "$ssh_probe" in
        *'Connection refused'*|*'onnection timed out'*|*'No route to host'*) ssh_listening='no' ;;
        *) ssh_listening='yes' ;;
    esac
fi

ssh_instructions() {
    case "$os_name" in
        Darwin)
            cat <<'MACOS'
Turn on Remote Login yourself -- it needs administrator rights, so this
fragment will not do it for you:

  System Settings > General > Sharing > Remote Login  (switch it on, and under
  the (i) button allow access for your own user)

The equivalent from a terminal, which will ask for your password:

  sudo systemsetup -setremotelogin on
MACOS
            ;;
        Linux)
            cat <<'LINUX'
Start an SSH server yourself -- it needs root, so this fragment will not do it
for you. On Debian/Ubuntu:

  sudo apt install openssh-server
  sudo systemctl enable --now ssh

On Fedora/RHEL/Arch:

  sudo dnf install openssh-server   # or: sudo pacman -S openssh
  sudo systemctl enable --now sshd

Then make sure the host firewall lets port 22 through from the fleet.
LINUX
            ;;
        *)
            cat <<'OTHER'
Start an SSH server on this machine (port 22) and let the fleet reach it. This
fragment will not start one for you.
OTHER
            ;;
    esac
}

# ------------------------------------------------------------- summary

say ''
say '--------------------------------------------------------------'
say "Stado offline invite for '$fleet_target'"
say '--------------------------------------------------------------'
say "  Fleet key ($key_type): $key_action in $authorized_keys"
say '  Nothing was installed here and no service was started.'
say '  This fragment held only a PUBLIC key: no private key was received,'
say '  generated or sent anywhere by it.'
say ''
case "$ssh_listening" in
    yes)
        say 'Remote login: an SSH server is answering on port 22.'
        ;;
    no)
        say 'Remote login: NOTHING is answering on port 22, so the fleet cannot'
        say 'reach this machine yet. Turn it on before the operator tries:'
        say ''
        ssh_instructions
        ;;
    *)
        say 'Remote login: could not be checked here (no nc, no ssh client). The'
        say 'fleet needs an SSH server answering on port 22:'
        say ''
        ssh_instructions
        ;;
esac
say ''
say "Send this line back to the operator (chosen as the $address_kind):"
say "$login_user@$address"
STADO_OFFLINE_INVITE
"##;

/// The offline fragment for one target, ready to paste.
///
/// Both substitutions land inside single quotes in `sh`, so a value containing
/// a quote would end the literal and turn the rest of the fragment into
/// something else entirely; a multi-line key line would smuggle extra
/// directives into `authorized_keys`. Neither can happen with what
/// [`crate::cli::fleet::key::authorized_keys_line`] produces from a minted
/// ed25519 key, which is why they are refusals and not escapes. Pure.
pub fn offline_snippet(target_name: &str, authorized_line: &str) -> Result<String, String> {
    let line = authorized_line.trim();
    if line.is_empty() {
        return Err("the minted key produced no authorized_keys line".to_string());
    }
    if line.contains('\n') || line.contains('\r') {
        return Err("the minted key produced more than one authorized_keys line".to_string());
    }
    if line.contains('\'') || target_name.contains('\'') {
        return Err(
            "the key line or target name contains a quote, which cannot go into the fragment"
                .to_string(),
        );
    }
    if target_name.is_empty()
        || !target_name
            .chars()
            .all(|letter| letter.is_ascii_alphanumeric() || matches!(letter, '.' | '_' | '-'))
    {
        return Err(format!(
            "target name '{target_name}' is not usable in the fragment"
        ));
    }
    Ok(OFFLINE_SNIPPET
        .replace("@FLEET_KEY@", line)
        .replace("@TARGET@", target_name))
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

/// `stado fleet invite [--name NAME] [--expires 24h] [--uses 1] [--offline]` —
/// mint the channel key for a machine nobody has touched yet, plus the thing
/// its owner has to run.
///
/// What that thing is depends on whether a control point can actually serve
/// `/join.sh`, which is probed here before anything is minted. Reachable: the
/// one line, exactly as before. Not reachable, for any of the three reasons
/// [`probe_checkpoint`] distinguishes: the offline fragment instead, with the
/// reason said out loud. `offline` skips the probe and takes that path on
/// purpose. The one-liner is never printed for an address that did not answer —
/// a command that cannot work is worse than no command, because the operator
/// finds out from the machine's owner.
///
/// The rest of the order is unchanged and deliberate: the key is minted before
/// the invite is recorded, because an invite whose key does not exist fails on
/// somebody else's laptop, and everything that can still refuse — recording the
/// object, building the fragment — removes that freshly minted credential item
/// again. A half-minted invite leaves nothing behind.
pub async fn invite(
    name: Option<&str>,
    expires: &str,
    uses: u64,
    offline: bool,
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
    let id = mint_id()?;
    let target_name = match name {
        Some(given) => given.to_string(),
        None => derived_target_name(&id),
    };
    preflight_invite_name(&document, &live, &target_name)?;

    // The control point comes from configuration or from an entrance this
    // fleet published — never from a name compiled into this binary. A built-in
    // default would be exactly the silent fallback that printed a one-liner for
    // a host nobody deployed.
    //
    // `enrollment.url` wins when it is set, because a written-down decision
    // outranks anything discovered here. Next comes the live
    // `enrollments/ingress.json`, the entrance `fleet ingress up` verified from
    // the internet — without it the one-line mode has nothing to point at on a
    // fleet with no public deployment. `api.url` stays the release/deployment
    // endpoint and is the last fallback, so a deployment that never configured
    // a separate enrollment origin behaves exactly as it did before. All three
    // empty still means `not_configured`.
    //
    // `--offline` does not consult the ingress: it probes nothing by definition,
    // and an unprobed tunnel address is not a base, it is a guess.
    let (base, base_source) = if offline {
        (enrollment_base(), BASE_FROM_ENROLLMENT_URL)
    } else {
        resolve_invite_base(&store).await
    };
    let checkpoint = if offline {
        Checkpoint {
            url: base.clone(),
            probed: false,
            reachable: false,
            reason: REASON_FORCED_OFFLINE,
            detail: "--offline was requested, so the control point was not probed".to_string(),
        }
    } else {
        probe_checkpoint(&base).await
    };
    let from_ingress = base_source == BASE_FROM_INGRESS;
    let mode = checkpoint.mode();
    // Offline mints no secret at all, rather than minting one and being trusted
    // to forget it.
    let secret = match mode {
        MODE_OFFLINE => None,
        _ => Some(mint_secret()?),
    };

    let runner = crate::deploy::production_runner();
    let (public_key, fingerprint) =
        crate::cli::fleet::key::rotate::generate_stored(&runner, &target_name).await?;
    let line = crate::cli::fleet::key::authorized_keys_line(
        &public_key,
        &crate::cli::fleet::key::item_id(&target_name),
    );
    let snippet = match mode {
        MODE_OFFLINE => match offline_snippet(&target_name, &line) {
            Ok(snippet) => Some(snippet),
            Err(detail) => {
                discard_minted_key(&target_name).await;
                return Err(format!(
                    "could not build the offline fragment ({detail}); the minted key for '{target_name}' was removed"
                ));
            }
        },
        _ => None,
    };

    let created_at = Utc::now();
    let invite = Invite {
        id: id.clone(),
        secret_sha256: secret.as_deref().map(secret_digest).unwrap_or_default(),
        target_name: target_name.clone(),
        created_at: created_at.to_rfc3339(),
        expires_at: (created_at + lifetime).to_rfc3339(),
        uses_allowed: uses,
        uses_spent: 0,
        status: STATUS_OPEN.to_string(),
        created_by: crate::providers::vast::system_hostname(),
        mode: mode.to_string(),
    };
    let recorded = store
        .create_text_if_absent(
            &invite_path(&id),
            &serde_json::to_string_pretty(&invite_document(&invite))
                .map_err(|exc| exc.to_string())?,
        )
        .await;
    match recorded {
        Ok(true) => {}
        Ok(false) | Err(_) => {
            let detail = match recorded {
                Err(exc) => exc.to_string(),
                _ => format!("invite id {id} already exists in the store"),
            };
            discard_minted_key(&target_name).await;
            return Err(format!(
                "could not record the invite ({detail}); the minted key for '{target_name}' was removed"
            ));
        }
    }

    let token = secret.as_deref().map(|secret| format!("{id}.{secret}"));
    let command = token
        .as_deref()
        .map(|token| join_command(&checkpoint.url, token));
    let next_step = format!("stado fleet enroll {target_name} --ssh <address> --bootstrap");
    if as_json {
        let mut rendered = json!({
            "id": invite.id,
            "mode": invite.mode,
            "target_name": invite.target_name,
            "created_at": invite.created_at,
            "expires_at": invite.expires_at,
            "uses_allowed": invite.uses_allowed,
            "public_key": public_key,
            "authorized_keys_line": line,
            "checkpoint": checkpoint_document(&checkpoint),
            "base_source": base_source,
            "base_is_temporary": from_ingress,
        });
        match (&token, &command, &snippet) {
            (Some(token), Some(command), _) => {
                rendered["token"] = Value::String(token.clone());
                rendered["token_shown_once"] = Value::Bool(true);
                rendered["join_command"] = Value::String(command.clone());
            }
            (_, _, Some(snippet)) => {
                rendered["snippet"] = Value::String(snippet.clone());
                rendered["snippet_is_not_a_secret"] = Value::Bool(true);
                rendered["next_step"] = Value::String(next_step.clone());
            }
            _ => {}
        }
        if from_ingress {
            rendered["base_warning"] = Value::String(format!(
                "{} is a temporary Cloudflare quick-tunnel address published by 'stado fleet \
                 ingress'; this one-liner stops working the moment that ingress is stopped, and a \
                 restarted ingress comes back under a different address",
                checkpoint.url
            ));
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&rendered).map_err(|exc| exc.to_string())?
        );
        return Ok(true);
    }
    println!(
        "invite {} for target '{}' (mode: {})",
        invite.id, invite.target_name, invite.mode
    );
    match (&token, &command, &snippet) {
        (Some(token), Some(command), _) => {
            println!("token: {token}");
            println!("  this is the only time the token is shown; nothing can reprint it");
            println!("expires: {} (uses allowed: {uses})", invite.expires_at);
            println!("channel key minted: {fingerprint}");
            println!("control point: {}", checkpoint.detail);
            println!("send this one line to the machine's owner:");
            println!("  {command}");
            println!(
                "then approve the machine: stado fleet pending, stado fleet approve <hostname>"
            );
            if from_ingress {
                println!(
                    "  that address is a TEMPORARY Cloudflare quick-tunnel address, published by \
                     'stado fleet ingress'."
                );
                println!(
                    "  the one line above stops working the moment the ingress is stopped, and a"
                );
                println!(
                    "  restarted ingress comes back under a DIFFERENT address — an invitation \
                     handed out"
                );
                println!(
                    "  before a restart is dead. Keep the ingress standing until the machine has \
                     joined,"
                );
                println!("  and check it with: stado fleet ingress status");
            }
        }
        (_, _, Some(snippet)) => {
            println!(
                "no token exists for an offline invite; there is nothing here to intercept or replay"
            );
            println!("expires: {} (uses allowed: {uses})", invite.expires_at);
            println!("channel key minted: {fingerprint}");
            if checkpoint.probed {
                println!("control point: {}", checkpoint.detail);
            } else {
                println!("control point not probed: {}", checkpoint.detail);
            }
            println!("switched to the offline invite method, which needs no HTTP route at all.");
            println!();
            println!(
                "paste everything between the two markers into a terminal ON THE MACHINE BEING ADDED:"
            );
            println!("----- 8< ----- stado offline invite for '{target_name}' ----- 8< -----");
            print!("{snippet}");
            println!("----- 8< ----- end of fragment ----- 8< -----");
            println!(
                "this fragment carries only the fleet's PUBLIC key, so it is not a secret: whoever reads it gains nothing."
            );
            println!("  the private half never leaves the operator's vault.");
            println!(
                "the owner runs it and sends back the user@address it prints on its last line."
            );
            println!("when that address arrives, run: {next_step}");
        }
        // Unreachable: online carries a token and a command, offline a
        // fragment, and the mode chose one of the two before the key was minted.
        _ => {
            return Err(
                "the invite was recorded but neither mode produced anything to send".to_string(),
            );
        }
    }
    Ok(true)
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

/// An open offline invite is not waiting for a redeemer, it is waiting for the
/// machine's owner to send back an address — nothing will ever spend it by
/// itself. The list says so where the state goes, because "open" alone reads
/// like the online mode, where somebody may be about to run the one-liner.
/// Pure.
pub fn status_label(invite: &Invite, status: &str) -> String {
    if invite.mode != MODE_OFFLINE {
        return status.to_string();
    }
    if status == STATUS_OPEN {
        "open (offline, awaiting address)".to_string()
    } else {
        format!("{status} (offline)")
    }
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
                    "mode": invite.mode,
                    "awaiting_address": invite.mode == MODE_OFFLINE && *status == STATUS_OPEN,
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
            status_label(invite, status),
            invite.uses_spent,
            invite.uses_allowed,
            invite.expires_at
        );
    }
    if found
        .iter()
        .any(|(invite, status)| invite.mode == MODE_OFFLINE && *status == STATUS_OPEN)
    {
        println!(
            "an offline invite closes when its machine is registered: stado fleet enroll NAME --ssh <address> --bootstrap"
        );
    }
    Ok(true)
}

/// Close the offline invite a fresh registration satisfied, if there is one.
///
/// Registering the name IS the redemption of an offline invite: it has no
/// secret and no route, so nothing else can ever spend it, and the operator
/// only gets to `fleet enroll` because the fragment installed the key and the
/// owner sent the address back. A revoked invite is left alone — revocation is
/// a deliberate refusal that enrolling the name by hand must not undo — and the
/// transition itself is [`mark_spent`], the very one `approve` drives for an
/// online invite.
///
/// Returns the id it closed, so the caller can say which one.
pub async fn close_offline_for_target(name: &str) -> Result<Option<String>, String> {
    let store = JobStorage::new().await.map_err(|exc| exc.to_string())?;
    let found = list_invites(&store).await?;
    let Some((invite, _)) = found.iter().find(|(invite, _)| {
        invite.mode == MODE_OFFLINE
            && invite.target_name == name
            && invite.status != STATUS_REVOKED
            && invite.status != STATUS_SPENT
    }) else {
        return Ok(None);
    };
    mark_spent(&store, &invite.id).await?;
    Ok(Some(invite.id.clone()))
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
    println!(
        "invite {id} for target '{}' is revoked (was {previous})",
        invite.target_name
    );
    println!(
        "the minted channel key is still in the credential store: stado fleet key rm '{}' removes it",
        invite.target_name
    );
    Ok(true)
}

