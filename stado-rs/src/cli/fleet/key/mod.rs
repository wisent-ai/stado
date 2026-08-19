//! SSH host keys in the globally selected credential store:
//! `key add|ls|rm|install|check`, plus generation and rotation in [`rotate`].
//!
//! Private material is never printed. A remote call reads the target key from
//! the selected store, writes one owner-only transient file for `ssh -i`, then
//! removes it. There is no OpenSSH home-directory fallback: changing
//! `STADO_CREDENTIALS_STORE` is a credential migration, not a second lookup
//! path.

mod channel;
pub use channel::channel_argv;
pub mod rotate;

use crate::deploy::{CommandSpec, Runner};
use crate::skarbiec::Client;
use serde_json::json;

/// Credential item id prefix for host keys; the target name follows it.
const ITEM_PREFIX: &str = "stado-ssh-";
/// Skarbiec's canonical kind for a private/public pair. `ssh-key` is an input
/// spelling, not a kind: the vault stores `private_key` and `public_key` as the
/// pair's fields and keeps the fingerprint and key type as context, and it
/// refuses a payload that claims any other kind.
const ITEM_TYPE: &str = "key-pair";

/// Credential item id for one target's host key.
pub fn item_id(target: &str) -> String {
    format!("{ITEM_PREFIX}{target}")
}

/// One `authorized_keys` line: the key's type and blob, then exactly one
/// comment.
///
/// The stored key still carries the comment `ssh-keygen -C` put on it, which is
/// already the credential item id, so pasting it verbatim in front of another
/// comment produced a line naming the same item twice — what `key install`
/// appends today. Only the first two fields of a public key are the key; the
/// rest is commentary, and this owns the commentary.
pub fn authorized_keys_line(public_key: &str, comment: &str) -> String {
    let mut fields = public_key.split_whitespace();
    match (fields.next(), fields.next()) {
        (Some(kind), Some(blob)) => format!("{kind} {blob} {comment}"),
        // Not a two-field key: pass it through rather than silently truncating
        // something the caller will have to recognize in an error message.
        _ => format!("{} {comment}", public_key.trim()),
    }
}

pub(crate) async fn run_checked(
    runner: &Runner,
    spec: CommandSpec,
    what: &str,
) -> Result<String, String> {
    let output = runner(spec).await?;
    if output.ok() {
        Ok(output.stdout)
    } else {
        Err(format!("{what} failed: {}", output.detail()))
    }
}

/// Key management is an operator action routed through the globally selected
/// credential store; Skarbiec uses the external admin bootstrap grant.
pub(crate) fn configured_client() -> Result<Client, String> {
    let credentials =
        crate::credential_store::admin_credentials().map_err(|exc| exc.to_string())?;
    Client::new(
        &credentials.url,
        &credentials.consumer,
        &credentials.token_file,
    )
    .map_err(|exc| exc.to_string())
}

/// Fields of a key-pair item the SSH channel's reader must be able to read.
/// Grants are per item, so these are exactly the capabilities a freshly minted
/// key is missing.
const CHANNEL_FIELDS: [&str; 2] = ["private_key", "public_key"];

/// Finish a key write: make the item readable by the consumer the SSH channel
/// reads it through, then prove it through that same consumer.
///
/// Two distinct stores are in play. An owner write reaches a vault FILE; the
/// channel reaches a BROKER, authenticating as the administrative consumer of
/// [`crate::credential_store::admin_credentials`]. Skarbiec authorizes reads per
/// item, so the write leaves the item invisible to that consumer until its grant
/// is widened — which is why every freshly minted key used to be dead on
/// arrival. And on a host whose broker forwards to another machine's vault, the
/// two stores are not the same store at all, so a key that looks written is
/// invisible to the fleet. Neither condition is detectable later from anywhere
/// nearer than the failing host, so the write is not finished until the reader
/// can see what was written.
///
/// `verify` names the fields read back and the values they must carry. Values
/// are compared, never printed.
pub(crate) async fn settle_readable(
    client: &Client,
    id: &str,
    verify: &[(&str, &str)],
) -> Result<(), String> {
    // A file store answers its owner directly and has no grants to widen; the
    // read-back there goes through the store, not through a broker that may not
    // exist on that deployment.
    let brokered = crate::credential_store::skarbiec_url().is_some();
    if brokered {
        let credentials =
            crate::credential_store::admin_credentials().map_err(|exc| exc.to_string())?;
        let outcome = crate::credential_store::grant::grant_field_reads(
            &credentials.consumer,
            std::path::Path::new(&credentials.token_file),
            id,
            &CHANNEL_FIELDS,
        )
        .map_err(|exc| {
            format!(
                "cannot make {id} readable by {}: {exc}",
                credentials.consumer
            )
        })?;
        if outcome.wrote() {
            // Progress, not output: `fleet invite --json` mints a channel key on
            // its way to printing one JSON document, and a widened grant is not
            // part of that document. stderr keeps the operator informed without
            // making every JSON consumer parse around it.
            eprintln!(
                "granted {} read on {} ({} capabilities held, was {})",
                credentials.consumer,
                outcome.added.join(", "),
                outcome.held_after,
                outcome.held_before
            );
        }
    }
    for (field, expected) in verify {
        let read = if brokered {
            client
                .read_field(id, field)
                .await
                .map(|value| value.as_str().map(str::to_string))
        } else {
            client.read_string(id, field).await
        };
        // Every way this can end badly says the same thing. The item was
        // written and its fields were granted a moment ago, so a reader that
        // refuses them, or answers with something else, is not reading the
        // vault this write reached — nothing the caller can fix by retrying or
        // by granting more.
        let reason = match read {
            Ok(stored) if stored.as_deref().map(str::trim) == Some(expected.trim()) => continue,
            Ok(Some(_)) => "a different value".to_string(),
            Ok(None) => "nothing".to_string(),
            Err(crate::skarbiec::SkarbiecError::Response { status, detail })
                if status == reqwest::StatusCode::FORBIDDEN.as_u16()
                    || status == reqwest::StatusCode::NOT_FOUND.as_u16() =>
            {
                format!("HTTP {status}: {}", detail.trim())
            }
            Err(error) => return Err(error.to_string()),
        };
        return Err(format!(
            "wrote {id} and granted its fields, but the reader that opens the channel serves \
             {reason} for {field}. This machine's vault is not the one the fleet reads: mint on \
             the host that holds it (`stado host vaults` names them), or point \
             SKARBIEC_VAULT_FILE at that vault"
        ));
    }
    Ok(())
}

/// `key add TARGET --from PATH` — move an existing private key into the
/// selected store. The source file is removed only after a read-back verifies
/// the stored material; private content is never printed.
pub async fn add(runner: &Runner, target: &str, from: &str) -> Result<bool, String> {
    let metadata = std::fs::symlink_metadata(from)
        .map_err(|exc| format!("cannot inspect key file {from}: {exc}"))?;
    if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
        return Err(format!(
            "key source {from} must be a regular file, not a symlink or special file"
        ));
    }
    let private_key = std::fs::read_to_string(from)
        .map_err(|exc| format!("cannot read key file {from}: {exc}"))?;
    let public_key = run_checked(
        runner,
        CommandSpec::new(vec![
            "ssh-keygen".to_string(),
            "-y".to_string(),
            "-f".to_string(),
            from.to_string(),
        ]),
        "ssh-keygen -y",
    )
    .await?;
    let fingerprint_line = run_checked(
        runner,
        CommandSpec::new(vec![
            "ssh-keygen".to_string(),
            "-lf".to_string(),
            from.to_string(),
        ]),
        "ssh-keygen -lf",
    )
    .await?;
    let fingerprint = fingerprint_line
        .split_whitespace()
        .find(|part| part.starts_with("SHA256:"))
        .unwrap_or_default()
        .to_string();
    let key_type = fingerprint_line
        .rsplit('(')
        .next()
        .map(|part| part.trim().trim_end_matches(')').to_string())
        .unwrap_or_default();
    let id = item_id(target);
    let client = configured_client()?;
    client
        .write_described(
            &id,
            ITEM_TYPE,
            &json!({
                "private_key": private_key.trim(),
                "public_key": public_key.trim(),
            }),
            &json!({
                "key_type": key_type,
                "fingerprint": fingerprint,
                "added_at": chrono::Utc::now().to_rfc3339(),
            }),
        )
        .await
        .map_err(|exc| exc.to_string())?;
    // The source file is about to be deleted, so the read-back is the only
    // thing standing between a half-written key and a key that exists nowhere.
    // It reads the material by name through the consumer the channel uses:
    // `fingerprint` is schema context rather than a field, carries no grant,
    // and proves nothing about whether this key can open a connection.
    if let Err(error) = settle_readable(
        &client,
        &id,
        &[
            ("private_key", private_key.trim()),
            ("public_key", public_key.trim()),
        ],
    )
    .await
    {
        let _ = client.delete_item(&id).await;
        return Err(format!(
            "credential item {id} failed read-back verification: {error}. The source file was \
             preserved"
        ));
    }
    if let Err(error) = std::fs::remove_file(from) {
        let rollback = client.delete_item(&id).await;
        return Err(match rollback {
            Ok(()) => format!(
                "cannot remove source key {from}: {error}; the credential-store write was rolled back"
            ),
            Err(rollback_error) => format!(
                "cannot remove source key {from}: {error}; store rollback also failed: {rollback_error}"
            ),
        });
    }
    let _ = std::fs::remove_file(format!("{from}.pub"));
    println!("moved key into credential item {id} ({fingerprint})");
    Ok(true)
}

/// `key ls` — metadata of every stored SSH host key. No private fields.
pub async fn ls() -> Result<bool, String> {
    let client = configured_client()?;
    let items = client.list_items().await.map_err(|exc| exc.to_string())?;
    let mut shown = Vec::new();
    for item in items {
        if !item.id.starts_with(ITEM_PREFIX) {
            continue;
        }
        // `fingerprint` and `key_type` are schema CONTEXT on a `key-pair`, not
        // fields: Skarbiec's canonical form keeps the two halves of the key as
        // fields and everything descriptive beside them. Asking for them as
        // fields is refused, and the refusal used to arrive here as two blank
        // columns, which reads as a key with no fingerprint rather than as a
        // read of the wrong place. The private field is never asked for.
        let context = client
            .read_field(&item.id, "context")
            .await
            .unwrap_or_else(|_| json!({}));
        let described = |name: &str| {
            context
                .get(name)
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default()
        };
        shown.push(format!(
            "{}\t{}\t{}",
            item.id,
            described("key_type"),
            described("fingerprint")
        ));
    }
    if shown.is_empty() {
        println!("no SSH host keys in the credential store");
    } else {
        for line in &shown {
            println!("{line}");
        }
    }
    Ok(true)
}

/// `key rm TARGET` — delete the target's SSH host key.
pub async fn rm(target: &str) -> Result<bool, String> {
    let client = configured_client()?;
    client
        .delete_item(&item_id(target))
        .await
        .map_err(|exc| exc.to_string())?;
    println!("removed credential item {}", item_id(target));
    Ok(true)
}

/// `key install TARGET` — append the stored public key to the target's
/// authorized_keys through the existing credential-store-backed channel.
pub async fn install(runner: &Runner, target: &str) -> Result<bool, String> {
    let client = configured_client()?;
    let public_key = client
        .read_string(&item_id(target), "public_key")
        .await
        .map_err(|exc| exc.to_string())?
        .ok_or_else(|| {
            format!(
                "credential item {} has no public_key field",
                item_id(target)
            )
        })?;
    let registry = crate::targets::load_registry_auto()
        .await
        .map_err(|exc| exc.to_string())?;
    let target_entry = registry
        .lookup(target)
        .ok_or_else(|| format!("target '{target}' not found in registry"))?;
    let destination = target_entry
        .ssh
        .as_deref()
        .ok_or_else(|| format!("target '{target}' has no remote channel (ssh=null)"))?;
    let line = authorized_keys_line(&public_key, &item_id(target));
    let command = format!(
        "mkdir -p \"$HOME/.ssh\" && touch \"$HOME/.ssh/authorized_keys\" && grep -qF '{line}' \"$HOME/.ssh/authorized_keys\" || echo '{line}' >> \"$HOME/.ssh/authorized_keys\""
    );
    let (argv, _key) = channel_argv(target, destination, &command).await?;
    run_checked(runner, CommandSpec::new(argv), "authorized_keys install").await?;
    println!("installed public key for '{target}' into authorized_keys on {destination}");
    Ok(true)
}

// ---------------------------------------------------------------------------
// First contact: the `adopt` enrollment method
// ---------------------------------------------------------------------------
//
// `key install` above and `install_first_contact` below append the same line to
// the same file, and they are NOT interchangeable — they differ in which key
// opens the session, and that is exactly the difference between "already in the
// fleet" and "not yet in the fleet":
//
// - `key install` rides [`channel_argv`], i.e. `ssh -i` with the target's
//   private key materialized from the credential store and `IdentitiesOnly`.
//   It therefore presupposes that this very key is ALREADY in the machine's
//   authorized_keys. That makes it the right tool for re-installing or
//   repairing a line on a machine the fleet can already reach (and it is what
//   `key rotate` uses to install the new key through the still-valid old one),
//   and useless for first contact: a machine that has never been adopted
//   rejects that key, so the command that would install it cannot run.
// - `install_first_contact` deliberately passes NO identity file. It runs plain
//   `ssh DEST`, so OpenSSH uses whatever the operator can already open the
//   machine with: a forwarded or unlocked agent, one of the operator's own
//   `~/.ssh` keys, or — with `BatchMode=no` — OpenSSH's own interactive
//   password prompt. Stado neither reads, stores, nor forwards a password: the
//   prompt is OpenSSH's, on its own `/dev/tty`, and no secret is ever placed in
//   argv. The direction of the key is unchanged and not negotiable: only the
//   PUBLIC half travels to the machine, the private half stays in the
//   operator's vault, because it is the fleet that connects TO the machine.

/// What one first-contact install did to the machine's authorized_keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdoptOutcome {
    /// The line was appended by this run.
    Installed,
    /// The line was already there; nothing was written.
    AlreadyPresent,
}

/// The remote half of `--install-key`, run by the machine's own login shell.
///
/// It reads the authorized_keys line from stdin rather than from argv, so the
/// line needs no shell quoting and never appears in a process listing, and it
/// reports through one marker line on stdout so that ssh's exit status stays a
/// pure transport/authentication signal: 0 with a marker is a verdict from the
/// machine, 3 is our own write failure, 255 is ssh's.
///
/// Idempotent by construction: the append happens only when an exact
/// whole-line match (`grep -qxF`) is absent, and the line is read back
/// afterwards, so a second run reports [`AdoptOutcome::AlreadyPresent`] instead
/// of duplicating the entry.
const FIRST_CONTACT_PROGRAM: &str = r#"set -u
line=$(cat)
umask 077
dir="$HOME/.ssh"
file="$dir/authorized_keys"
mkdir -p "$dir" 2>/dev/null || { echo "STADO_ADOPT_WRITE_FAILED cannot create $dir"; exit 3; }
chmod 700 "$dir" 2>/dev/null
touch "$file" 2>/dev/null || { echo "STADO_ADOPT_WRITE_FAILED cannot create $file"; exit 3; }
chmod 600 "$file" 2>/dev/null
if grep -qxF "$line" "$file" 2>/dev/null; then echo STADO_ADOPT_PRESENT; exit 0; fi
printf '%s\n' "$line" >> "$file" 2>/dev/null || { echo "STADO_ADOPT_WRITE_FAILED cannot append to $file"; exit 3; }
grep -qxF "$line" "$file" 2>/dev/null || { echo "STADO_ADOPT_WRITE_FAILED $file does not carry the line after the append"; exit 3; }
echo STADO_ADOPT_INSTALLED
"#;

/// One SSH invocation for first contact: no identity file, so OpenSSH resolves
/// the credential itself, and `BatchMode=no` so it may ask the operator.
///
/// `ConnectTimeout` bounds the one failure that would otherwise hang forever (a
/// filtered port), and `NumberOfPasswordPrompts` bounds the retries. There is
/// deliberately no wall-clock timeout on the command: a human at a password
/// prompt is not a stalled process.
fn first_contact_argv(destination: &str) -> Vec<String> {
    [
        "ssh",
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-o",
        "BatchMode=no",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "NumberOfPasswordPrompts=3",
        destination,
        FIRST_CONTACT_PROGRAM,
    ]
    .iter()
    .map(|part| (*part).to_string())
    .collect()
}

/// ssh diagnostics that mean the TCP/DNS leg never completed, so no
/// credential was ever offered.
const UNREACHABLE_MARKERS: [&str; 12] = [
    "connection refused",
    "connection timed out",
    "operation timed out",
    "no route to host",
    "network is unreachable",
    "host is down",
    "could not resolve hostname",
    "name or service not known",
    "nodename nor servname",
    "temporary failure in name resolution",
    "no address associated with hostname",
    "connection closed by remote host",
];

/// ssh diagnostics that mean the server was reached and refused the operator.
const REJECTED_MARKERS: [&str; 6] = [
    "permission denied",
    "too many authentication failures",
    "no supported authentication methods",
    "authentications that can continue",
    "host key verification failed",
    "remote host identification has changed",
];

fn matches_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

/// Turn one failed first-contact run into the sentence naming the operator's
/// next action. The three failures below need three different actions, which is
/// why they are three different messages and not one "ssh failed":
/// dial the machine, fix the credential, or fix the machine's home directory.
fn first_contact_failure(destination: &str, output: &crate::deploy::CommandOutput) -> String {
    // `accept-new` records an unknown host key and says so on stderr. That
    // notice is not a diagnosis of anything, and quoting it ahead of the real
    // one buries the sentence the operator has to read.
    let diagnostic = output
        .detail()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("Warning: Permanently added"))
        .collect::<Vec<_>>()
        .join("; ");
    let lowered = diagnostic.to_ascii_lowercase();
    if output.code == 255 && matches_any(&lowered, &UNREACHABLE_MARKERS) {
        return format!(
            "no SSH connection to {destination} was established, so no credential was even offered: \
             check the address and port, that the machine is awake and on this network, and that \
             sshd is listening there. ssh said: {diagnostic}"
        );
    }
    if output.code == 255 && matches_any(&lowered, &REJECTED_MARKERS) {
        return format!(
            "{destination} answered on SSH and rejected the authentication: --install-key can only \
             use a session you can already open yourself, so unlock or forward an agent \
             (ssh-add -l), or let OpenSSH ask for the account password — which needs a terminal, \
             not a script or the dashboard. ssh said: {diagnostic}"
        );
    }
    if output.code == 255 {
        return format!(
            "ssh could not open a session to {destination}; it reported neither an unreachable \
             address nor a rejected credential: {diagnostic}"
        );
    }
    let reason = output
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("STADO_ADOPT_WRITE_FAILED "))
        .map(str::to_string)
        .unwrap_or_else(|| {
            if !diagnostic.is_empty() {
                diagnostic.clone()
            } else if output.code == 0 {
                "the machine ran the install and confirmed nothing".to_string()
            } else {
                format!("the remote program exited {}", output.code)
            }
        });
    format!(
        "authentication to {destination} succeeded, but writing ~/.ssh/authorized_keys there \
         failed: {reason}. The account and the credential are fine; its home directory is not \
         writable — a full disk, a read-only or wrongly owned home, or a login shell that cannot \
         run a command. Fix that on the machine and re-run; the install is idempotent."
    )
}

/// The target's public key, minted on demand.
///
/// A missing pair is minted through the one existing mint, [`rotate::generate`],
/// which is also the only thing that grants the channel's reader and proves the
/// grant by reading back — so this never becomes a second, subtly different
/// mint. The read-back here is through the same reader again, which is what
/// makes "there is a usable key" a fact before anything is sent to the machine.
async fn ensure_public_key(runner: &Runner, target: &str) -> Result<String, String> {
    let id = item_id(target);
    let stored = configured_client()?
        .read_string(&id, "public_key")
        .await
        .map_err(|exc| exc.to_string())?;
    if let Some(public_key) = stored.filter(|value| !value.trim().is_empty()) {
        return Ok(public_key.trim().to_string());
    }
    println!("no key pair for '{target}' yet; minting one into {id}");
    rotate::generate(runner, target).await?;
    configured_client()?
        .read_string(&id, "public_key")
        .await
        .map_err(|exc| exc.to_string())?
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
        .ok_or_else(|| format!("credential item {id} was minted without a public_key field"))
}

/// `fleet enroll NAME --ssh DEST --install-key` — the `adopt` method's first
/// contact: put the fleet's PUBLIC key into the machine's authorized_keys over
/// a session the operator can already open by other means, so that every later
/// step (the identity probe, the registry write, the optional bootstrap) rides
/// the fleet's own key like any other registered machine.
///
/// Sends the public half only, and only over stdin. Returns what it did, so the
/// caller can say whether anything changed.
pub async fn install_first_contact(
    runner: &Runner,
    target: &str,
    destination: &str,
) -> Result<AdoptOutcome, String> {
    let public_key = ensure_public_key(runner, target).await?;
    let line = authorized_keys_line(&public_key, &item_id(target));
    let spec = CommandSpec {
        argv: first_contact_argv(destination),
        stdin: Some(format!("{line}\n")),
        timeout: None,
    };
    let output = runner(spec).await?;
    if !output.ok() {
        return Err(first_contact_failure(destination, &output));
    }
    if output
        .stdout
        .lines()
        .any(|line| line == "STADO_ADOPT_PRESENT")
    {
        println!(
            "the fleet public key for '{target}' is already in ~/.ssh/authorized_keys on {destination}; nothing appended"
        );
        return Ok(AdoptOutcome::AlreadyPresent);
    }
    if output
        .stdout
        .lines()
        .any(|line| line == "STADO_ADOPT_INSTALLED")
    {
        println!(
            "installed the fleet public key for '{target}' into ~/.ssh/authorized_keys on {destination}"
        );
        return Ok(AdoptOutcome::Installed);
    }
    Err(first_contact_failure(destination, &output))
}

/// `key check TARGET` — verify the selected-store key opens the channel.
pub async fn check(runner: &Runner, target: &str) -> Result<bool, String> {
    let registry = crate::targets::load_registry_auto()
        .await
        .map_err(|exc| exc.to_string())?;
    let target_entry = registry
        .lookup(target)
        .ok_or_else(|| format!("target '{target}' not found in registry"))?;
    let destination = target_entry
        .ssh
        .as_deref()
        .ok_or_else(|| format!("target '{target}' has no remote channel (ssh=null)"))?;
    let (argv, _key) = channel_argv(target, destination, "hostname").await?;
    let answered = run_checked(runner, CommandSpec::new(argv), "hostname over the channel").await?;
    println!(
        "credential-store key verified: {destination} answered as {}",
        answered.trim()
    );
    Ok(true)
}
