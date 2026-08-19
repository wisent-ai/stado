//! `stado release quarantine ...` — list the digests one host refuses to roll
//! out again, and retire exactly one of them.
//!
//! The release agent quarantines a digest that failed to become ready and then
//! never tries it again, which is correct: a candidate that dies in ninety
//! seconds must not be respawned in a loop. What was missing was the way back.
//! Until this command existed there were exactly two: hand-edit
//! `<state_dir>/<product>.json` on the host, or publish a new version number so
//! the digest changes. The operator refused both, and both deserve refusing —
//! the first is an unaudited write to the file a rollout is driven from, and the
//! second burns a version to say "try again".
//!
//! What this is not: `clear` starts nothing, restarts nothing and kills nothing.
//! It removes one map entry. The agent's next tick reads the state file, finds
//! the desired digest no longer quarantined, and rolls it out on its own — the
//! same path it would have taken had the digest never failed.

use chrono::Utc;
use clap::{Args, Subcommand};
use serde_json::json;

use crate::deploy::{host_channel, production_runner, shlex_quote};
use crate::release_agent;
use crate::release_control::{
    sha256_bytes, ProductReleasePolicy, ReleaseControl, ReleaseTargetPolicy,
};
use crate::targets::ComputeTarget;

use super::CmdError;

#[derive(Subcommand)]
pub enum QuarantineCommands {
    /// List the digests this host will not roll out again.
    List(QuarantineListArgs),
    /// Retire exactly one quarantined digest so the agent retries it.
    Clear(QuarantineClearArgs),
}

#[derive(Args)]
pub struct QuarantineListArgs {
    pub product: String,
    /// Registry target name. Optional only while the product rolls out to one.
    #[arg(long)]
    target: Option<String>,
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
pub struct QuarantineClearArgs {
    pub product: String,
    /// Registry target name. Never inferred: this command rewrites that host's
    /// rollout state, and the host is not a detail to guess at.
    #[arg(long)]
    target: String,
    /// The exact quarantined artifact digest, as `quarantine list` prints it.
    #[arg(long)]
    digest: String,
    /// Why this digest is being given another chance. Required, recorded in the
    /// audit trail beside the state file, and never defaulted.
    #[arg(long)]
    reason: String,
    #[arg(long)]
    json: bool,
}

/// The most one remote read brings back.
///
/// A rollout state document is a single line of a few kilobytes. The cap is
/// here for the log tails this channel also carries, and [`remote_read`] treats
/// exceeding it as an error rather than a truncation: a state file read short
/// and then rewritten from the short read would strand the rollout it was meant
/// to unstick.
pub(crate) const REMOTE_READ_LIMIT_BYTES: u64 = 1 << 20;

/// Read one file on a registry host. Absent and empty are different answers:
/// `Ok(None)` is "there is no such file", `Ok(Some(""))` is "the file is there
/// and has nothing in it", and an operator draws opposite conclusions from
/// those two. The content comes back base64 so a line of the file cannot forge
/// one of the markers that frame it.
const READ_TEMPLATE: &str = r#"set -eu
path=@PATH@
if [ ! -f "$path" ]; then
  printf 'STADO_QUARANTINE_ABSENT\t%s\n' "$path"
  exit 0
fi
printf 'STADO_QUARANTINE_BYTES\t%s\n' "$(/usr/bin/wc -c < "$path" | /usr/bin/tr -d ' ')"
printf 'STADO_QUARANTINE_BASE64\t%s\n' "$(@BODY@ | /usr/bin/openssl base64 -A)"
"#;

const READ_WHOLE_BODY: &str = r#"/usr/bin/head -c @LIMIT@ "$path""#;

/// Replace one quarantine map, and leave the evidence that it happened.
///
/// Every guard here exists because the file being rewritten is the one the
/// release agent drives from, and the agent is writing it too, every tick:
///
/// - the live file's digest must still be the digest this command read, or some
///   other writer moved and this write would discard their work;
/// - the previous bytes are copied to a timestamped backup before anything is
///   written, so the state before the change is recoverable without this tool;
/// - the audit trail is proven appendable before the state is touched, because
///   an unaudited mutation is worse than a refused one;
/// - the staged document is hashed *after* it lands on the host's disk and
///   compared against what this command built, so a short or interrupted
///   transfer is discarded instead of renamed over a working rollout;
/// - only then does `mv` publish it, atomically, within one directory.
///
/// The staging file is a copy of the live one truncated in place, so the mode
/// and owner the agent gave its state file survive the rewrite.
const CLEAR_TEMPLATE: &str = r#"set -euo pipefail
state=@STATE@
backup=@BACKUP@
staging=@STAGING@
audit=@AUDIT@
expected_live=@EXPECTED_LIVE@
expected_next=@EXPECTED_NEXT@

if [ ! -f "$state" ]; then
  printf '%s\n' "rollout state $state is missing" >&2
  exit 1
fi
if [ -e "$backup" ]; then
  printf '%s\n' "state backup $backup already exists" >&2
  exit 1
fi
if ! : >> "$audit"; then
  printf '%s\n' "cannot append to the quarantine audit trail $audit" >&2
  exit 1
fi
/bin/chmod u=rw,go= "$audit"
line=$(/usr/bin/openssl dgst -sha256 -r "$state")
live=${line%% *}
if [ "$live" != "$expected_live" ]; then
  printf '%s\n' "rollout state changed while this command ran: read $expected_live, found $live" >&2
  exit 1
fi
/bin/cp -p "$state" "$backup"
printf 'STADO_QUARANTINE\tbackup\t%s\n' "$backup"
/bin/rm -f "$staging"
/bin/cp -p "$state" "$staging"
printf '%s' @DOCUMENT@ | /usr/bin/openssl base64 -d -A > "$staging"
line=$(/usr/bin/openssl dgst -sha256 -r "$staging")
staged=${line%% *}
if [ "$staged" != "$expected_next" ]; then
  # The state was never changed, so the backup is a duplicate of the live file.
  # Leaving it behind would make an immediate retry refuse on its own debris.
  /bin/rm -f "$staging" "$backup"
  printf '%s\n' "staged rollout state reads back as $staged, not the $expected_next this command built" >&2
  exit 1
fi
/bin/mv "$staging" "$state"
printf 'STADO_QUARANTINE\tcommitted\t%s\n' "$state"
printf '%s' @RECORD@ | /usr/bin/openssl base64 -d -A >> "$audit"
printf '\n' >> "$audit"
printf 'STADO_QUARANTINE\taudited\t%s\n' "$audit"
"#;

/// Splice compile-time constants into a fixed remote program. Values are
/// shell-quoted by the caller; nothing operator-supplied reaches the shell
/// unquoted.
fn splice(template: &str, marks: &[(&str, &str)]) -> String {
    marks.iter().fold(template.to_string(), |script, (mark, value)| {
        script.replace(mark, value)
    })
}

/// The registry's release control plane, or the reason there is none.
pub(crate) async fn canonical_control() -> Result<ReleaseControl, CmdError> {
    let document = super::registry::fetch_document().await?;
    crate::release_control::control(&document)
        .map_err(CmdError::click)?
        .ok_or_else(|| CmdError::click("registry.release_control is not configured"))
}

/// The product policy and one of its targets.
///
/// `--target` may be omitted only while the product rolls out to exactly one
/// host. Guessing among several would put a write on whichever host sorted
/// first, which is the kind of help nobody asked for.
pub(crate) fn resolve_target<'a>(
    control: &'a ReleaseControl,
    product: &str,
    target: Option<&str>,
) -> Result<(String, &'a ProductReleasePolicy, &'a ReleaseTargetPolicy), CmdError> {
    let policy = control
        .products
        .get(product)
        .ok_or_else(|| CmdError::click(format!("unknown release product {product:?}")))?;
    let name = match target {
        Some(named) => named.to_string(),
        None => {
            let mut names = policy.targets.keys();
            match (names.next(), names.next()) {
                (Some(only), None) => only.clone(),
                (Some(_), Some(_)) => {
                    let declared: Vec<&str> =
                        policy.targets.keys().map(String::as_str).collect();
                    return Err(CmdError::usage(format!(
                        "{product} rolls out to {}; name one with --target",
                        declared.join(", ")
                    )));
                }
                _ => {
                    return Err(CmdError::click(format!(
                        "{product} declares no release target"
                    )))
                }
            }
        }
    };
    let target_policy = policy
        .targets
        .get(&name)
        .ok_or_else(|| CmdError::click(format!("{product} does not roll out to {name:?}")))?;
    Ok((name, policy, target_policy))
}

/// The registry-authorized host behind a release target name.
pub(crate) async fn compute_target(name: &str) -> Result<ComputeTarget, CmdError> {
    host_channel::canonical_target(name)
        .await
        .map_err(|error| CmdError::click(error.to_string()))
}

/// What the host said about one file: the full size it reported, and the bytes
/// it sent.
struct RemoteFile {
    bytes: u64,
    content: Vec<u8>,
}

async fn read_remote(
    host: &ComputeTarget,
    path: &str,
    body: &str,
) -> Result<Option<RemoteFile>, CmdError> {
    let script = splice(
        READ_TEMPLATE,
        &[("@PATH@", &shlex_quote(path)), ("@BODY@", body)],
    );
    let runner = production_runner();
    let output = host_channel::run_script(host, &script, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{}: cannot read {path}: {}",
            host.name,
            host_channel::last_error_line(&output, "remote read failed")
        )));
    }
    let mut bytes: Option<u64> = None;
    let mut encoded: Option<&str> = None;
    for line in output.stdout.lines() {
        let fields = host_channel::marker_fields(line);
        match fields.first().copied() {
            Some("STADO_QUARANTINE_ABSENT") => return Ok(None),
            Some("STADO_QUARANTINE_BYTES") => {
                bytes = fields.get(1).and_then(|value| value.parse().ok());
            }
            Some("STADO_QUARANTINE_BASE64") => encoded = Some(fields.get(1).copied().unwrap_or("")),
            _ => {}
        }
    }
    let (Some(bytes), Some(encoded)) = (bytes, encoded) else {
        return Err(CmdError::click(format!(
            "{}: answered nothing usable about {path}",
            host.name
        )));
    };
    let content = if encoded.is_empty() {
        Vec::new()
    } else {
        use base64::engine::general_purpose::STANDARD as BASE64;
        use base64::Engine;
        BASE64
            .decode(encoded)
            .map_err(|error| CmdError::click(format!("{}: {path} came back unreadable: {error}", host.name)))?
    };
    Ok(Some(RemoteFile { bytes, content }))
}

/// One whole file from a registry host, refused rather than truncated when it
/// exceeds [`REMOTE_READ_LIMIT_BYTES`].
pub(crate) async fn remote_read(
    host: &ComputeTarget,
    path: &str,
) -> Result<Option<String>, CmdError> {
    let body = READ_WHOLE_BODY.replace("@LIMIT@", &REMOTE_READ_LIMIT_BYTES.to_string());
    let Some(file) = read_remote(host, path, &body).await? else {
        return Ok(None);
    };
    if file.bytes > REMOTE_READ_LIMIT_BYTES {
        return Err(CmdError::click(format!(
            "{}: {path} is {} bytes, over the {REMOTE_READ_LIMIT_BYTES}-byte read limit",
            host.name, file.bytes
        )));
    }
    String::from_utf8(file.content)
        .map(Some)
        .map_err(|error| CmdError::click(format!("{}: {path} is not valid UTF-8: {error}", host.name)))
}

/// One host's rollout state for one product, identity-checked against the host
/// it came from.
///
/// `Ok(None)` is "the release agent has never reconciled this product here" —
/// not "everything is fine". A caller that folds the two together answers the
/// operator's question with the wrong half of the truth.
pub(crate) async fn remote_host_state(
    host: &ComputeTarget,
    state_dir: &str,
    product: &str,
) -> Result<Option<release_agent::HostReleaseState>, CmdError> {
    let path = release_agent::host_state_path(state_dir, product);
    let Some(payload) = remote_read(host, &path).await? else {
        return Ok(None);
    };
    release_agent::parse_state_document(payload.as_bytes(), product, &host.name, &path)
        .map(Some)
        .map_err(CmdError::click)
}

/// The digest the registry currently wants on this target's platform.
fn desired_digest<'a>(
    policy: &'a ProductReleasePolicy,
    target: &ReleaseTargetPolicy,
) -> Option<&'a str> {
    policy
        .desired
        .as_ref()?
        .artifacts
        .get(&target.platform)
        .map(|artifact| artifact.artifact_sha256.as_str())
}

/// `$USER`, the identity every other operator-initiated record in this CLI is
/// stamped with.
fn actor() -> String {
    std::env::var("USER").unwrap_or_else(|_| "operator".to_string())
}

/// The append-only record of every quarantine an operator retired, beside the
/// state it changed.
///
/// This area had no audit trail because it had no mutating command: the only
/// way to clear a quarantine was an editor on the host, which leaves nothing
/// behind at all. One JSONL line per clear, next to the document it changed, so
/// the next reader of that state file finds the account of why it looks the way
/// it does without leaving the directory.
fn audit_path(state_dir: &str, product: &str) -> String {
    format!("{state_dir}/{product}.quarantine-audit.jsonl")
}

/// A filename-safe instant, so a backup sorts by age and never collides with
/// the one before it.
fn stamp() -> String {
    Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

async fn list(args: &QuarantineListArgs) -> Result<(), CmdError> {
    let control = canonical_control().await?;
    let (target_name, policy, target_policy) =
        resolve_target(&control, &args.product, args.target.as_deref())?;
    let desired = desired_digest(policy, target_policy);
    let path = release_agent::host_state_path(&target_policy.state_dir, &args.product);
    let host = compute_target(&target_name).await?;
    let state = remote_host_state(&host, &target_policy.state_dir, &args.product).await?;
    let mut entries = Vec::new();
    if let Some(state) = state.as_ref() {
        for (digest, record) in &state.quarantined {
            entries.push(json!({
                "digest": digest,
                "reason": record.reason,
                "quarantined_at": record.quarantined_at.to_rfc3339(),
                "is_desired_digest": desired == Some(digest.as_str()),
            }));
        }
    }
    let report = json!({
        "product": args.product,
        "target": target_name,
        "entries": entries,
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }
    if state.is_none() {
        // "Nobody looked" printed as "nothing is there" is the one rendering
        // this must never produce: an absent state file means the agent has
        // never reconciled this product here, which is a different problem.
        println!("{target_name} has no rollout state at {path}");
        return Ok(());
    }
    if entries.is_empty() {
        println!("{} on {target_name}: nothing quarantined", args.product);
        return Ok(());
    }
    let rows: Vec<Vec<String>> = entries
        .iter()
        .map(|entry| {
            vec![
                entry["digest"].as_str().unwrap_or("-").to_string(),
                if entry["is_desired_digest"] == json!(true) {
                    "desired".to_string()
                } else {
                    "-".to_string()
                },
                entry["quarantined_at"].as_str().unwrap_or("-").to_string(),
                entry["reason"]
                    .as_str()
                    .unwrap_or("-")
                    .lines()
                    .next()
                    .unwrap_or("-")
                    .to_string(),
            ]
        })
        .collect();
    super::table::print(&["DIGEST", "ROLE", "QUARANTINED AT", "REASON"], &rows);
    Ok(())
}

async fn clear(args: &QuarantineClearArgs) -> Result<(), CmdError> {
    let reason = args.reason.trim();
    if reason.is_empty() {
        return Err(CmdError::usage(
            "--reason must say why this digest is being retried",
        ));
    }
    let digest = args.digest.trim().to_ascii_lowercase();
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(CmdError::usage(
            "--digest must be the 64-character sha256 hex digest quarantine list prints",
        ));
    }
    let control = canonical_control().await?;
    let (target_name, _, target_policy) =
        resolve_target(&control, &args.product, Some(&args.target))?;
    let path = release_agent::host_state_path(&target_policy.state_dir, &args.product);
    let host = compute_target(&target_name).await?;
    let payload = remote_read(&host, &path).await?.ok_or_else(|| {
        CmdError::click(format!(
            "{target_name} has no rollout state at {path}: nothing is quarantined there"
        ))
    })?;
    let mut state = release_agent::parse_state_document(
        payload.as_bytes(),
        &args.product,
        &target_name,
        &path,
    )
    .map_err(CmdError::click)?;
    let Some(record) = state.quarantined.remove(&digest) else {
        return Err(CmdError::click(format!(
            "{digest} is not quarantined for {} on {target_name}",
            args.product
        )));
    };
    // `phase` and `updated_at` stay exactly as the agent left them. They are
    // the agent's account of its own last tick, and a tick is precisely what
    // this command does not perform; rewriting them would have `release status`
    // report a reconciliation that never ran.
    let document = release_agent::state_document_bytes(&state).map_err(CmdError::click)?;
    let audited_at = Utc::now();
    let audit = audit_path(&target_policy.state_dir, &args.product);
    let backup = format!("{path}.quarantine-backup-{}", stamp());
    let staging = format!(
        "{}/.{}.json.stado-quarantine-{}",
        target_policy.state_dir,
        args.product,
        uuid::Uuid::new_v4().simple()
    );
    // The agent's own reason and timestamp go into the record because clearing
    // the entry deletes them from the state file, and an audit trail that
    // destroys the evidence for the change it documents is decoration.
    let mut line = serde_json::to_vec(&json!({
        "actor": actor(),
        "host": target_name,
        "product": args.product,
        "digest": digest,
        "reason": reason,
        "audited_at": audited_at.to_rfc3339(),
        "quarantine_reason": record.reason,
        "quarantined_at": record.quarantined_at.to_rfc3339(),
        "state_backup": backup,
    }))?;
    // A newline inside the record would split one clear across two JSONL rows.
    line.retain(|byte| *byte != b'\n');
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine;
    let script = splice(
        CLEAR_TEMPLATE,
        &[
            ("@STATE@", &shlex_quote(&path)),
            ("@BACKUP@", &shlex_quote(&backup)),
            ("@STAGING@", &shlex_quote(&staging)),
            ("@AUDIT@", &shlex_quote(&audit)),
            ("@EXPECTED_LIVE@", &shlex_quote(&sha256_bytes(payload.as_bytes()))),
            ("@EXPECTED_NEXT@", &shlex_quote(&sha256_bytes(&document))),
            ("@DOCUMENT@", &shlex_quote(&BASE64.encode(&document))),
            ("@RECORD@", &shlex_quote(&BASE64.encode(&line))),
        ],
    );
    let runner = production_runner();
    let output = host_channel::run_script(&host, &script, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{target_name}: rollout state was not changed: {}",
            host_channel::last_error_line(&output, "remote quarantine clear failed")
        )));
    }
    let committed = output
        .stdout
        .lines()
        .any(|line| host_channel::marker_fields(line).get(1) == Some(&"committed"));
    if !committed {
        return Err(CmdError::click(format!(
            "{target_name}: host exited clean without confirming the rewrite of {path}"
        )));
    }
    let report = json!({
        "product": args.product,
        "target": target_name,
        "digest": digest,
        "cleared": true,
        "reason": reason,
        "audited_at": audited_at.to_rfc3339(),
        "state_backup": backup,
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "cleared {digest} for {} on {target_name}\n  it was quarantined at {} because: {}\n  previous state backed up to {backup}\n  audited in {audit}\n  nothing was started, stopped or restarted; the release agent rolls this digest out on its next tick",
            args.product,
            record.quarantined_at.to_rfc3339(),
            record.reason.lines().next().unwrap_or(&record.reason),
        );
    }
    Ok(())
}

pub async fn dispatch(command: QuarantineCommands) -> Result<(), CmdError> {
    match command {
        QuarantineCommands::List(args) => list(&args).await,
        QuarantineCommands::Clear(args) => clear(&args).await,
    }
}
