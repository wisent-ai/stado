//! A role's identity kept in a keychain Stado owns on the installing machine.
//!
//! A recipe that declares `signing_role` beside `signing_keychain` signs its
//! source installations with that role's identity from that keychain. Stado
//! keeps the keychain itself, at every installation: it reads the role's
//! certificate and key from the vault, and when the keychain does not hold
//! that certificate (a new Mac, a renewed certificate) it imports it, so the
//! installed program's identity never changes because a machine or a
//! certificate did. Before this, the only kept keychain was the operator's
//! login keychain, which held his own identity and not the fleet's, and every
//! source install of Skarbiec on a Mac was refused as an identity change.

use super::super::core::command;
use super::keychain::{hold_signing_lock, keep_apple_issuers, signing_home};
use crate::common::atomic_write;
use anyhow::{bail, Context, Result};
use std::path::Path;

/// What keeping did, for the receipt: `kept` (the keychain already held the
/// role's certificate), `imported` (it did not, and now does) or
/// `unrefreshed` (the role could not be read, as while Skarbiec itself is
/// down for its own installation, and the keychain kept earlier is signed
/// from as it is; `fingerprint` is then unknown and the installed program's
/// identity is chosen from it).
pub struct Kept {
    pub outcome: &'static str,
    pub fingerprint: Option<String>,
    pub cause: Option<String>,
}

/// Keep `role`'s identity in `keychain`, which must be under Stado's signing
/// directory: a keychain elsewhere is the operator's, and Stado never writes
/// into it. Refused naming the role and keychain when the role cannot be
/// read, its certificate and key do not match, or an import fails.
pub fn keep(role: &str, keychain: &Path) -> Result<Kept> {
    let home = signing_home()?;
    if keychain.parent() != Some(home.as_path()) {
        bail!(
            "signing_keychain {} is not in Stado's signing directory {}, so Stado does not keep \
             role {role} in it; declare $HOME/.stado/signing/<name>.keychain-db beside signing_role",
            keychain.display(),
            home.display()
        );
    }
    let read = |field: &str| {
        super::secret(role, field).with_context(|| {
            format!("keeping role {role}'s identity in {}", keychain.display())
        })
    };
    let (certificate, private_key) = match read("certificate").and_then(|c| Ok((c, read("private_key")?))) {
        Ok(pair) => pair,
        Err(error) if keychain.exists() => {
            return Ok(Kept { outcome: "unrefreshed", fingerprint: None, cause: Some(format!("{error:#}")) })
        }
        Err(error) => return Err(error.context(format!(
            "{} holds no identity yet, so nothing can be signed until role {role} can be read",
            keychain.display()
        ))),
    };
    let chain = super::keychain::blocks(&certificate)?;
    let _lock = hold_signing_lock(&home)?;
    let staging = home.join(format!(".kept-{}", uuid::Uuid::new_v4()));
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&staging)?;
    let outcome = import(role, keychain, &staging, &chain, &private_key);
    let removed = std::fs::remove_dir_all(&staging);
    let kept = outcome?;
    removed.with_context(|| format!("removing the staged identity {}", staging.display()))?;
    Ok(kept)
}

fn import(
    role: &str,
    keychain: &Path,
    staging: &Path,
    chain: &[String],
    private_key: &str,
) -> Result<Kept> {
    let certificate_path = staging.join("certificate.pem");
    let key_path = staging.join("private-key.pem");
    atomic_write(&certificate_path, chain[0].as_bytes())?;
    atomic_write(&key_path, private_key.as_bytes())?;
    let cert = certificate_path.to_str().context("non-UTF8 credential path")?;
    let key = key_path.to_str().context("non-UTF8 credential path")?;
    let public = |args: &[&str]| -> Result<Vec<u8>> { Ok(command("/usr/bin/openssl", args, true)?.stdout) };
    if public(&["x509", "-in", cert, "-pubkey", "-noout"])? != public(&["pkey", "-in", key, "-pubout"])? {
        bail!("role {role}'s certificate and private key do not match; nothing was kept");
    }
    let fingerprint = String::from_utf8(
        command("/usr/bin/openssl", &["x509", "-in", cert, "-fingerprint", "-sha1", "-noout"], true)?.stdout,
    )?
    .trim()
    .split_once('=')
    .context("certificate fingerprint missing")?
    .1
    .replace(':', "");
    let path = keychain.to_str().context("non-UTF8 keychain path")?;
    if !keychain.exists() {
        command("/usr/bin/security", &["create-keychain", "-p", "", path], true)?;
        command("/usr/bin/security", &["set-keychain-settings", path], true)?;
    }
    command("/usr/bin/security", &["unlock-keychain", "-p", "", path], true)?;
    if chain.len() > 1 {
        keep_apple_issuers(&chain[1..].concat())?;
    }
    let listing = String::from_utf8(
        command("/usr/bin/security", &["find-identity", "-v", "-p", "codesigning", path], true)?.stdout,
    )?;
    if listing.contains(&fingerprint) {
        return Ok(Kept { outcome: "kept", fingerprint: Some(fingerprint), cause: None });
    }
    command("/usr/bin/security", &["import", cert, "-k", path], true)?;
    command(
        "/usr/bin/security",
        &["import", key, "-k", path, "-P", "", "-T", "/usr/bin/codesign"],
        true,
    )?;
    command(
        "/usr/bin/security",
        &["set-key-partition-list", "-S", "apple-tool:,apple:,codesign:", "-s", "-k", "", path],
        true,
    )?;
    let listing = String::from_utf8(
        command("/usr/bin/security", &["find-identity", "-v", "-p", "codesigning", path], true)?.stdout,
    )?;
    if !listing.contains(&fingerprint) {
        bail!(
            "role {role}'s identity {fingerprint} was imported into {path}, which still lists no \
             valid codesigning identity with it: {}",
            listing.trim()
        );
    }
    Ok(Kept { outcome: "imported", fingerprint: Some(fingerprint), cause: None })
}
