//! Authorize one registry host's SERVICE RESOLVER to read the registry from
//! the service-directory authority.
//!
//! NO Python original. This module exists because of a real, six-day outage on
//! `ubuntu-server-rtx-pro-6000`.
//!
//! A resolver on the authority host itself reads the canonical store directly.
//! Every other host cannot: [`crate::cli::resolver::snapshot_source`] returns
//! `SnapshotSource::Authority` for it, and the only transport that answers is
//! `ssh <authority ssh> stado resolver snapshot`, opened with the resolver's own
//! key (`$HOME/.stado/resolver-ssh-key`, or `STADO_RESOLVER_SSH_KEY_FILE`). If
//! that key is absent, or present but not in the authority account's
//! `authorized_keys`, the resolver never obtains a snapshot — and a resolver
//! without a snapshot never reaches `bind_loopback`, so it binds NONE of its
//! declared adapters and publishes nothing at all.
//!
//! That is a silent failure with a loud log and no command behind it. The RTX
//! host's resolver had logged
//! `registry authority exited with exit status: 255: charles@100.120.25.24:
//! Permission denied (publickey,password,keyboard-interactive)` on 8,900
//! consecutive attempts. `stado fleet key` was no help: every verb there is
//! about the key the CONTROL PLANE uses to reach a target, and this is a
//! host-to-host hop between two registry hosts, in which the control plane is
//! neither end. There was no Stado command for it, so there was no legitimate
//! way to repair it at all — raw `ssh` to a fleet host is not one.
//!
//! What this does, and nothing else: mint the resolver keypair on TARGET if it
//! has none, read its PUBLIC half, and append that one line to the authority
//! account's `authorized_keys`. Both hops go through the audited host channel.
//! The private half is generated on the host that will use it and never leaves
//! it; the public half is the only thing that travels, in the direction the
//! resolver connects.
//!
//! Idempotent by construction: the key is generated only when absent, and the
//! line is appended only when an exact whole-line match is missing, so a second
//! run reports `already_present` and writes nothing.
//!
//! Authorizing the key is necessary and was not sufficient, and the second
//! blocker is worth naming here because it presents identically. Once the RTX
//! host could authenticate, `resolver snapshot` on the authority answered
//! `Error: primary and backup resolve to the same store (stado://probierz)`,
//! from `queue::storage::JobStorage::with_configured_read_failover`. The
//! authority declared `storage.backup.backend = stado` behind a `stado`
//! primary, and for the Stado object adapter `queue::copy::Endpoint::describe`
//! reads a namespace that is global to the process and ignores every
//! per-endpoint locator — so two `stado` endpoints are the same store by
//! construction and the guard can never pass. That killed the snapshot for
//! EVERY non-authority resolver on the fleet, not only this one, and it
//! surfaced as an unattributable `error_code="unknown"` at
//! `failure_point="cli.resolver.snapshot"`. The fleet's declared value is
//! `local`, which both other hosts already carried along with the
//! `~/.stado/local-backup` path the authority also already had; it was
//! one-host drift, repaired with
//! `stado host config-set <authority> storage.backup.backend local`.

use serde_json::{json, Value};

use super::{host_channel, production_runner, CommandOutput, DeployError};

/// Where a resolver looks for its own key, matching the default in
/// [`crate::cli::resolver`]'s `ssh_command`. An operator who has pointed a
/// resolver at another file through `STADO_RESOLVER_SSH_KEY_FILE` is not
/// served by this command, and is told so rather than handed a second key
/// that nothing reads.
const RESOLVER_KEY_FILE: &str = "$HOME/.stado/resolver-ssh-key";

/// Mint the resolver key if TARGET has none, then print its public half.
///
/// `ssh-keygen -N ''` for an unencrypted key is deliberate and is the only
/// shape that works: the resolver runs unattended under launchd/systemd and
/// has no way to answer a passphrase prompt. The file is created under
/// `umask 077` and its mode asserted afterwards, because OpenSSH refuses a
/// group-readable private key and that refusal would look exactly like the
/// authorization failure this command exists to end.
const MINT_PROGRAM: &str = r#"set -u
umask 077
key="$HOME/.stado/resolver-ssh-key"
mkdir -p "$HOME/.stado" || { echo "STADO_RESOLVER_KEY_FAILED cannot create $HOME/.stado"; exit 3; }
if [ ! -f "$key" ]; then
    rm -f "$key.pub"
    ssh-keygen -q -t ed25519 -N '' -C "stado-resolver@$(hostname)" -f "$key" \
        || { echo "STADO_RESOLVER_KEY_FAILED ssh-keygen did not produce $key"; exit 3; }
    echo STADO_RESOLVER_KEY_MINTED
else
    echo STADO_RESOLVER_KEY_PRESENT
fi
chmod 600 "$key" 2>/dev/null
if [ ! -f "$key.pub" ]; then
    ssh-keygen -y -f "$key" > "$key.pub" \
        || { echo "STADO_RESOLVER_KEY_FAILED cannot derive the public half of $key"; exit 3; }
fi
chmod 644 "$key.pub" 2>/dev/null
printf 'STADO_RESOLVER_KEY_PUBLIC '
cat "$key.pub"
"#;

/// Append one `authorized_keys` line on the authority host, once.
///
/// The line arrives on stdin, not in argv: it needs no shell quoting and never
/// appears in a process listing. The append is read back afterwards, so
/// "installed" is an observation rather than an assumption.
const AUTHORIZE_PROGRAM: &str = r#"set -u
line=$(cat)
case "$line" in
    "ssh-ed25519 "*|"ssh-rsa "*|"ecdsa-sha2-"*) ;;
    *) echo "STADO_RESOLVER_KEY_FAILED refusing a line that is not an ssh public key"; exit 3 ;;
esac
umask 077
dir="$HOME/.ssh"
file="$dir/authorized_keys"
mkdir -p "$dir" 2>/dev/null || { echo "STADO_RESOLVER_KEY_FAILED cannot create $dir"; exit 3; }
chmod 700 "$dir" 2>/dev/null
touch "$file" 2>/dev/null || { echo "STADO_RESOLVER_KEY_FAILED cannot create $file"; exit 3; }
chmod 600 "$file" 2>/dev/null
if grep -qxF "$line" "$file" 2>/dev/null; then echo STADO_RESOLVER_KEY_AUTHORIZED_ALREADY; exit 0; fi
if [ -s "$file" ] && [ -n "$(tail -c 1 "$file")" ]; then printf '\n' >> "$file"; fi
printf '%s\n' "$line" >> "$file" 2>/dev/null \
    || { echo "STADO_RESOLVER_KEY_FAILED cannot append to $file"; exit 3; }
grep -qxF "$line" "$file" 2>/dev/null \
    || { echo "STADO_RESOLVER_KEY_FAILED $file does not carry the line after the append"; exit 3; }
echo STADO_RESOLVER_KEY_AUTHORIZED
"#;

fn marker(output: &CommandOutput, prefix: &str) -> Option<String> {
    output
        .stdout
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix(prefix))
        .map(str::to_string)
}

fn refused(output: &CommandOutput, fallback: &str) -> DeployError {
    let named = marker(output, "STADO_RESOLVER_KEY_FAILED ");
    let stderr = output.stderr.trim();
    DeployError(match (named, stderr.is_empty()) {
        (Some(detail), _) => detail,
        (None, false) => stderr.to_string(),
        (None, true) => fallback.to_string(),
    })
}

/// The single public key TARGET's resolver will present, minting it if needed.
async fn resolver_public_key(
    target: &crate::targets::ComputeTarget,
) -> Result<(String, bool), DeployError> {
    let output = host_channel::run_script(target, MINT_PROGRAM, &production_runner()).await?;
    if !output.ok() {
        return Err(refused(
            &output,
            "the resolver key could not be established on the target",
        ));
    }
    let minted = output
        .stdout
        .lines()
        .any(|line| line.trim() == "STADO_RESOLVER_KEY_MINTED");
    let public_key = marker(&output, "STADO_RESOLVER_KEY_PUBLIC ")
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
        .ok_or_else(|| {
            DeployError(format!(
                "{}: the target reported no resolver public key",
                target.name
            ))
        })?;
    Ok((public_key, minted))
}

/// Authorize TARGET's resolver on the service-directory authority host.
pub async fn authorize(target_name: &str) -> Result<Value, DeployError> {
    let registry = host_channel::canonical_registry().await?;
    let directory = registry.service_directory.as_ref().ok_or_else(|| {
        DeployError("the canonical registry carries no service directory".to_string())
    })?;
    let authority_name = directory.authority.target.clone();
    // The authority's resolver reads its own store: `snapshot_source` returns
    // `SnapshotSource::Local` for it and opens no SSH session at all. Writing a
    // key for a hop that does not exist would leave an authorization nothing
    // uses, which is the kind of declaration this fleet has been bitten by.
    if authority_name == target_name {
        return Err(DeployError(format!(
            "{target_name:?} IS the service-directory authority, so its resolver reads the \
             canonical store directly and opens no session to authorize"
        )));
    }
    let target = host_channel::resolve_target(&registry, target_name)?.clone();
    let authority = host_channel::resolve_target(&registry, &authority_name)?.clone();

    let (public_key, minted) = resolver_public_key(&target).await?;
    // The program is a compile-time constant in argv and the key rides stdin:
    // `run_script` already spends stdin on the script itself, so a script that
    // also needs an argument has to arrive this way round.
    let (authorized, used_connection) = host_channel::run_program_with_stdin_and_connection(
        &authority,
        &["/bin/bash", "-c", AUTHORIZE_PROGRAM],
        &public_key,
        &production_runner(),
    )
    .await?;
    let (connection_path, authority_destination) = match used_connection {
        host_channel::UsedConnection::Local => ("local", "local process"),
        host_channel::UsedConnection::Ssh(path) => (path.name, path.destination),
    };
    if !authorized.ok() {
        return Err(refused(
            &authorized,
            "the authority host refused the authorized_keys append",
        ));
    }
    let state = if authorized
        .stdout
        .lines()
        .any(|line| line.trim() == "STADO_RESOLVER_KEY_AUTHORIZED_ALREADY")
    {
        "already_present"
    } else {
        "authorized"
    };

    Ok(json!({
        "target": target.name,
        "authority": authority_name,
        "authority_destination": authority_destination,
        "connection_path": connection_path,
        "key_file": RESOLVER_KEY_FILE,
        "key_state": if minted { "minted" } else { "present" },
        "key_type": public_key.split_whitespace().next().unwrap_or_default(),
        "authorized_keys": state,
    }))
}
