//! First contact: the `adopt` enrollment method.

use crate::cli::CmdError;
use crate::deploy::{CommandOutput, Runner};
use crate::primitives::failure::FailureCode;

use super::super::rotate;
use super::super::store::{authorized_keys_line, configured_client, item_id, CHANNEL_FIELDS};

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
/// pure transport/authentication signal:
/// 0 with a marker is a verdict from the
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
detail=$(mkdir -p "$dir" 2>&1) || { echo "STADO_ADOPT_WRITE_FAILED cannot create $dir: $detail"; exit 3; }
detail=$(chmod 700 "$dir" 2>&1) || { echo "STADO_ADOPT_WRITE_FAILED cannot make $dir owner-only: $detail"; exit 3; }
detail=$(touch "$file" 2>&1) || { echo "STADO_ADOPT_WRITE_FAILED cannot create $file: $detail"; exit 3; }
detail=$(chmod 600 "$file" 2>&1) || { echo "STADO_ADOPT_WRITE_FAILED cannot make $file owner-only: $detail"; exit 3; }
if grep -qxF "$line" "$file"; then echo STADO_ADOPT_PRESENT; exit 0; fi
detail=$(printf '%s\n' "$line" 2>&1 >> "$file") || { echo "STADO_ADOPT_WRITE_FAILED cannot append to $file: $detail"; exit 3; }
grep -qxF "$line" "$file" || { echo "STADO_ADOPT_WRITE_FAILED $file does not carry the line after the append"; exit 3; }
echo STADO_ADOPT_INSTALLED
"#;

/// Run first contact's ssh with `line` on its stdin, in this process's own
/// process group, never through the deploy runner, which starts every child
/// as the leader of a fresh group: a child
/// outside the terminal's foreground group that opens `/dev/tty` for the
/// password prompt is stopped by the kernel (SIGTTOU), and the enroll then
/// waited on a stopped ssh with no prompt shown and nothing that would ever
/// end it.
async fn run_first_contact(destination: &str, line: String) -> Result<CommandOutput, CmdError> {
    let argv = first_contact_argv(destination);
    let started = tokio::task::spawn_blocking(move || {
        let mut command = std::process::Command::new(&argv[0]);
        command.args(&argv[1..]);
        crate::wait::output_with_input(&mut command, line.as_bytes())
    })
    .await
    .map_err(|exc| CmdError::unreachable(format!("ssh to {destination} did not run: {exc}")))?;
    let output = started.map_err(|exc| {
        CmdError::unreachable(format!("ssh to {destination} could not be started: {exc}"))
    })?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let code = output.status.code().ok_or_else(|| {
        CmdError::unreachable(format!(
            "ssh to {destination} ended with {} and returned no exit code; stderr: {}",
            output.status,
            stderr.trim()
        ))
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    Ok(CommandOutput { code, stdout, stderr })
}

/// One SSH invocation for first contact: no identity file, so OpenSSH resolves
/// the credential itself, and `BatchMode=no` so it may ask the operator.
///
/// `NumberOfPasswordPrompts` bounds the password retries. The command itself
/// runs until ssh exits: a human at a password prompt is not a stalled
/// process, and an unreachable port ends with the network's own error.
fn first_contact_argv(destination: &str) -> Vec<String> {
    [
        "ssh",
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-o",
        "BatchMode=no",
        "-o",
        "NumberOfPasswordPrompts=3",
        destination,
        FIRST_CONTACT_PROGRAM,
    ]
    .iter()
    .map(|part| (*part).to_string())
    .collect()
}

/// Turn one failed first-contact run into the sentence naming the operator's
/// next action. ssh exits 255 when it could not open the session at all — the
/// machine was not reached, or it was reached and refused the credential —
/// and the two are told apart by ssh's own sentence, quoted whole, not by a
/// list of phrases, so that failure states no class. Any other status means
/// the session opened and the install on the machine failed: the machine's
/// home is the outage, infra_down.
fn first_contact_failure(destination: &str, output: &crate::deploy::CommandOutput) -> CmdError {
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
    if output.code == 255 {
        return CmdError::unreachable(format!(
            "ssh could not open a session to {destination} (exit 255). ssh said: {diagnostic}. \
             If it never connected, check the address and port, that the machine is awake and on \
             this network, and that sshd is listening there. If it connected and refused the \
             credential, --install-key can only use a session you can already open yourself: \
             unlock or forward an agent (ssh-add -l), or let OpenSSH ask for the account password \
             — which needs a terminal, not a script or the dashboard."
        ));
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
    CmdError::click(format!(
        "authentication to {destination} succeeded, but writing ~/.ssh/authorized_keys there \
         failed: {reason}. The account and the credential are fine; its home directory is not \
         writable — a full disk, a read-only or wrongly owned home, or a login shell that cannot \
         run a command. Fix that on the machine and re-run; the install is idempotent."
    ))
    .stating(FailureCode::InfraDown)
}

/// The target's public key, minted on demand.
///
/// A missing pair is minted through the one existing mint, [`rotate::generate`],
/// which is also the only thing that grants the channel's reader and proves the
/// grant by reading back — so this never becomes a second, subtly different
/// mint. The read-back here is through the same reader again, which is what
/// makes "there is a usable key" a fact before anything is sent to the machine.
async fn ensure_public_key(runner: &Runner, target: &str) -> Result<String, CmdError> {
    let id = item_id(target);
    let stored = stored_public_key(&id).await?;
    if let Some(public_key) = stored.filter(|value| !value.trim().is_empty()) {
        return Ok(public_key.trim().to_string());
    }
    // Narration of a step inside `fleet enroll`, on standard error: standard
    // output belongs to the command's answer, which may be one JSON document.
    eprintln!("no key pair for '{target}' yet; minting one into {id}");
    rotate::generate_stored(runner, target).await?;
    configured_client()?
        .read_declared_string(&id, "public_key")
        .await?
        .filter(|value| !value.trim().is_empty())
        .map(|value| value.trim().to_string())
        .ok_or_else(|| {
            CmdError::declaration(format!(
                "credential item {id} was minted without a public_key field"
            ))
        })
}

/// The public half the channel's reader can read; `None` when the vault
/// owner holds no such item, so the caller mints one.
///
/// Skarbiec answers a consumer the same 403 for an item it may not read and
/// for an item that does not exist, so it does not reveal which items exist.
/// Read as a refusal, that 403 stopped every enroll of a machine that had no
/// key yet, which is exactly the machine `--install-key` is for. The vault
/// owner's own listing tells the two apart: an absent item is minted; a
/// present one is a mint cut off between storing the item and granting the
/// channel's reader (a killed `fleet invite create`, a lost host channel), and
/// that idempotent grant, naming only this target's own channel item, is
/// completed and the read made again. A reader still refused after it is the
/// refusal that stands.
async fn stored_public_key(id: &str) -> Result<Option<String>, CmdError> {
    let client = configured_client()?;
    let refused = match client.read_declared_string(id, "public_key").await {
        Ok(stored) => return Ok(stored),
        Err(error) if error.status() == Some(reqwest::StatusCode::FORBIDDEN.as_u16()) => error,
        Err(error) => return Err(CmdError::from(error)),
    };
    let (owner, _) = crate::cli::release_catalog::fleet_hosts().await?;
    let held = crate::cli::host::owner_item_ids(&owner)
        .await
        .map_err(|listing| {
            listing.within(format!(
                "the fleet's reader was refused {id} ({refused}), and the vault owner {owner}'s \
                 item list, which says whether it exists, could not be read"
            ))
        })?;
    if !held.contains(id) {
        eprintln!(
            "{id} is not in the vault owner {owner}'s item list (the fleet's reader was \
             refused it: {refused})"
        );
        return Ok(None);
    }
    eprintln!(
        "{id} is in the vault owner {owner}'s vault, but the fleet's reader may not read it \
         ({refused}): a mint stopped before granting it, so the grant is completed now"
    );
    crate::cli::host::settle_consumer_reads(id, CHANNEL_FIELDS)
        .await
        .map_err(|grant| grant.within(format!("{id} could not be made readable")))?;
    Ok(client.read_declared_string(id, "public_key").await?)
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
) -> Result<AdoptOutcome, CmdError> {
    let public_key = ensure_public_key(runner, target).await?;
    let line = authorized_keys_line(&public_key, &item_id(target));
    let output = run_first_contact(destination, format!("{line}\n")).await?;
    if !output.ok() {
        return Err(first_contact_failure(destination, &output));
    }
    if output
        .stdout
        .lines()
        .any(|line| line == "STADO_ADOPT_PRESENT")
    {
        eprintln!(
            "the fleet public key for '{target}' is already in ~/.ssh/authorized_keys on {destination}; nothing appended"
        );
        return Ok(AdoptOutcome::AlreadyPresent);
    }
    if output
        .stdout
        .lines()
        .any(|line| line == "STADO_ADOPT_INSTALLED")
    {
        eprintln!(
            "installed the fleet public key for '{target}' into ~/.ssh/authorized_keys on {destination}"
        );
        return Ok(AdoptOutcome::Installed);
    }
    Err(first_contact_failure(destination, &output))
}
