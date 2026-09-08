//! First contact: the `adopt` enrollment method.

use crate::deploy::{CommandSpec, Runner};

use super::super::rotate;
use super::super::store::{authorized_keys_line, configured_client, item_id};

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
