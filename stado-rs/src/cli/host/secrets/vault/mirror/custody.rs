//! `stado credentials token custody-local OPERATION VAULT CONSUMER FILE`: the
//! host-side half of `stado credentials token sync`, run by each host's
//! installed Stado. No vault or grant is ever written.
//!
//! `export` reads the consumer's grant from VAULT and the bearer from FILE,
//! checks that the bearer hashes to the grant, and prints both. `install`,
//! `check`, `install-shared` and `check-shared` read that export on stdin and
//! verify it against the destination's own grant (or, shared, the owner's),
//! then write the bearer to FILE owner-only and atomically unless checking.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::cli::CmdError;

/// Skarbiec's fixed-token validation bound, not deployment tuning.
const TOKEN_LIMIT: usize = 4096;
const OWNER_ONLY: u32 = 0o600;
const PRIVATE_BITS: u32 = 0o077;

fn refused(detail: impl std::fmt::Display) -> CmdError {
    CmdError::click(format!("token custody refused: {detail}"))
}

fn token_bytes(data: &[u8]) -> Result<Vec<u8>, CmdError> {
    let text = std::str::from_utf8(data).map_err(refused)?;
    let token = text.trim_end_matches(['\r', '\n']);
    if token.is_empty() || token.len() > TOKEN_LIMIT || token.chars().any(char::is_whitespace) {
        return Err(refused(
            "token file must contain one bounded non-whitespace token",
        ));
    }
    Ok(token.as_bytes().to_vec())
}

fn token_path(value: &str) -> Result<PathBuf, CmdError> {
    let home = fs::canonicalize(std::env::var("HOME").map_err(|_| refused("HOME is not set"))?)
        .map_err(refused)?;
    let expanded = ["~/", "$HOME/"]
        .iter()
        .find_map(|prefix| value.strip_prefix(prefix))
        .map(|rest| home.join(rest))
        .unwrap_or_else(|| PathBuf::from(value));
    if !expanded.is_absolute() {
        return Err(refused(
            "token file must be an absolute or home-relative path",
        ));
    }
    let name = expanded
        .file_name()
        .ok_or_else(|| refused("token file names no file"))?;
    let parent = fs::canonicalize(expanded.parent().unwrap_or(Path::new("/"))).map_err(refused)?;
    if !parent.starts_with(&home) {
        return Err(refused(
            "token file must resolve inside the host account's home",
        ));
    }
    Ok(parent.join(name))
}

/// The bearer in PATH, or `None` when the file does not exist.
fn read_token(path: &Path) -> Result<Option<Vec<u8>>, CmdError> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(path);
    let mut file = match file {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(refused(error)),
    };
    let metadata = file.metadata().map_err(refused)?;
    let euid = unsafe { nix::libc::geteuid() };
    if !metadata.is_file() || metadata.uid() != euid || metadata.mode() & PRIVATE_BITS != 0 {
        return Err(refused(
            "token file must be an owner-controlled regular file",
        ));
    }
    let mut data = Vec::new();
    let bound = (TOKEN_LIMIT + "\r\n".len() + 1) as u64;
    std::io::Read::by_ref(&mut file)
        .take(bound)
        .read_to_end(&mut data)
        .map_err(refused)?;
    if data.len() as u64 >= bound {
        return Err(refused(
            "token file exceeds the bounded token and line ending",
        ));
    }
    token_bytes(&data).map(Some)
}

fn grant_at(vault: &str, consumer: &str) -> Result<(String, Value), CmdError> {
    let document: Value =
        serde_json::from_slice(&fs::read(vault).map_err(refused)?).map_err(refused)?;
    let owner = document["owner"]
        .as_str()
        .filter(|owner| !owner.is_empty())
        .ok_or_else(|| refused("declared vault has no owner"))?
        .to_string();
    let grant = document["tokens"][consumer].clone();
    if !grant.is_object() {
        return Err(refused(format!(
            "declared vault has no grant for {consumer}"
        )));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(refused)?
        .as_secs();
    if !grant["expires_at"]
        .as_u64()
        .is_some_and(|expiry| expiry > now)
    {
        return Err(refused(format!(
            "declared grant for {consumer} is expired or has no expiry"
        )));
    }
    if !grant["capabilities"].is_array() {
        return Err(refused(format!(
            "declared grant for {consumer} has no capability set"
        )));
    }
    Ok((owner, grant))
}

fn verify_token(token: &[u8], grant: &Value) -> Result<(), CmdError> {
    if Some(hex::encode(Sha256::digest(token)).as_str()) != grant["hash"].as_str() {
        return Err(refused(
            "token file does not match the declared consumer grant",
        ));
    }
    Ok(())
}

fn export(vault: &str, consumer: &str, file: &str) -> Result<Value, CmdError> {
    let (owner, grant) = grant_at(vault, consumer)?;
    let path = token_path(file)?;
    let token = read_token(&path)?.ok_or_else(|| refused("source token file does not exist"))?;
    verify_token(&token, &grant)?;
    let token = String::from_utf8(token).map_err(refused)?;
    Ok(json!({"owner": owner, "grant": grant, "token": token,
        "source_token_file": path.display().to_string()}))
}

fn write_atomically(path: &Path, token: &[u8]) -> Result<(), CmdError> {
    let parent = path
        .parent()
        .ok_or_else(|| refused("token file has no parent"))?;
    let staged = parent.join(format!(".stado-token-sync-{}", std::process::id()));
    let written = (|| -> std::io::Result<()> {
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(OWNER_ONLY)
            .open(&staged)?;
        output.set_permissions(fs::Permissions::from_mode(OWNER_ONLY))?;
        output.write_all(token)?;
        output.sync_all()?;
        fs::rename(&staged, path)
    })();
    if written.is_err() {
        let _ = fs::remove_file(&staged);
    }
    written.map_err(refused)
}

fn install(
    vault: &str,
    consumer: &str,
    file: &str,
    check: bool,
    shared: bool,
) -> Result<Value, CmdError> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(refused)?;
    let source: Value = serde_json::from_str(&input).map_err(refused)?;
    // A host that reads the owner's vault through its resolver route has no
    // authoritative copy; the owner's grant, verified at export, is the one.
    let current_grant = || -> Result<(String, Value), CmdError> {
        if shared {
            let owner = source["owner"].as_str().unwrap_or_default().to_string();
            Ok((owner, source["grant"].clone()))
        } else {
            grant_at(vault, consumer)
        }
    };
    let (owner, grant) = current_grant()?;
    if Some(owner.as_str()) != source["owner"].as_str() || grant != source["grant"] {
        return Err(refused("destination vault differs from source owner or consumer grant; synchronize the vault first"));
    }
    let token = token_bytes(source["token"].as_str().unwrap_or_default().as_bytes())?;
    verify_token(&token, &grant)?;
    let path = token_path(file)?;
    let current = read_token(&path)?;
    if check && current.as_deref() != Some(token.as_slice()) {
        return Err(refused(
            "destination token file is missing or does not match the declared consumer grant",
        ));
    }
    let changed = current.as_deref() != Some(token.as_slice());
    if changed {
        if current_grant()? != (owner.clone(), grant.clone()) {
            return Err(refused("destination grant changed during token delivery"));
        }
        write_atomically(&path, &token)?;
    }
    let delivered = read_token(&path)?.ok_or_else(|| refused("delivered token file vanished"))?;
    verify_token(&delivered, &grant)?;
    if current_grant()? != (owner, grant.clone()) {
        return Err(refused(
            "destination grant changed after token delivery; delivered bearer is not verified",
        ));
    }
    let status = if check {
        "token_checked"
    } else if changed {
        "token_synced"
    } else {
        "token_unchanged"
    };
    Ok(json!({
        "status": status,
        "changed": changed,
        "skarbiec": {
            "ok": true,
            "consumer": consumer,
            "token_file": path.display().to_string(),
            "audience": grant["audience"],
            "expires_at": grant["expires_at"],
            "capabilities": grant["capabilities"],
            "workload_bound": grant["workload_public_key"].as_str().is_some_and(|key| !key.is_empty()),
        },
        "source_token_file": source["source_token_file"],
        "detail": "Bearer verified against the unchanged declared grant; no vault was written.",
    }))
}

/// Run one custody operation and print its JSON result.
pub fn custody_local(
    operation: &str,
    vault: &str,
    consumer: &str,
    file: &str,
) -> Result<(), CmdError> {
    let result = match operation {
        "export" => export(vault, consumer, file)?,
        "install" | "check" | "install-shared" | "check-shared" => install(
            vault,
            consumer,
            file,
            operation.starts_with("check"),
            operation.ends_with("-shared"),
        )?,
        other => return Err(refused(format!("unknown token custody operation {other}"))),
    };
    println!("{result}");
    Ok(())
}
