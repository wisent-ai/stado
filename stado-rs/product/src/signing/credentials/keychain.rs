//! What signing scopes on one host share: the user's keychain search list,
//! the signing directory and its lock, and the keychain of Apple's issuers.

use super::super::{
    constants::{BEGIN_CERTIFICATE, END_CERTIFICATE},
    core::command,
};
use crate::common::atomic_write;
use anyhow::{bail, Context, Result};
use base64::Engine;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

/// Each distinct PEM certificate of `text`, in order: the leaf first, then
/// the issuers that came with it. Refused when `text` carries none or a
/// block does not decode.
pub(super) fn blocks(text: &str) -> Result<Vec<String>> {
    let mut blocks = Vec::new();
    let mut seen = BTreeSet::new();
    for part in text.split(BEGIN_CERTIFICATE).skip(1) {
        let (body, _) = part
            .split_once(END_CERTIFICATE)
            .context("supplied certificate has an unterminated PEM block")?;
        let encoded: Vec<_> = body
            .bytes()
            .filter(|byte| !byte.is_ascii_whitespace())
            .collect();
        let certificate = base64::engine::general_purpose::STANDARD
            .decode(&encoded)
            .context("supplied certificate has invalid PEM encoding")?;
        if seen.insert(certificate) {
            blocks.push(format!("{BEGIN_CERTIFICATE}{body}{END_CERTIFICATE}\n"));
        }
    }
    if blocks.is_empty() {
        bail!("supplied signing certificate carries no PEM certificate");
    }
    Ok(blocks)
}

fn listed() -> Result<Vec<String>> {
    Ok(String::from_utf8(
        command("/usr/bin/security", &["list-keychains", "-d", "user"], true)?.stdout,
    )?
    .lines()
    .map(|line| line.trim().trim_matches('"').to_owned())
    .filter(|line| !line.is_empty())
    .collect())
}

/// Put `keychain` first on the user's keychain search list, reading the list
/// as it is now. Signing jobs on one host share that list: a scope that
/// restored a snapshot taken when it opened dropped every keychain another
/// job had added since, and that job's `codesign` then built no chain for
/// its identity and failed with `errSecInternalComponent`. The
/// same restores left entries naming temporary keychains already deleted;
/// an entry whose file is gone names nothing to search and is not kept.
pub(super) fn put_first(keychain: &str) -> Result<()> {
    let current = listed()?;
    let kept: Vec<&str> = current
        .iter()
        .map(String::as_str)
        .filter(|listed| *listed != keychain && Path::new(listed).exists())
        .collect();
    if current.first().map(String::as_str) == Some(keychain) && kept.len() + 1 == current.len() {
        return Ok(());
    }
    let mut arguments = vec!["list-keychains", "-d", "user", "-s", keychain];
    arguments.extend(kept);
    command("/usr/bin/security", &arguments, true)?;
    Ok(())
}

/// Take `keychain` off the user's keychain search list, keeping every other
/// entry as the list holds it now, whoever added it.
pub(super) fn take_off(keychain: &str) -> Result<()> {
    let current = listed()?;
    if !current.iter().any(|listed| listed == keychain) {
        return Ok(());
    }
    let mut arguments = vec!["list-keychains", "-d", "user", "-s"];
    arguments.extend(
        current
            .iter()
            .map(String::as_str)
            .filter(|listed| *listed != keychain),
    );
    command("/usr/bin/security", &arguments, true)?;
    Ok(())
}

/// The directory every signing scope on this host makes its private
/// identity directory in, `$HOME/.stado/signing`, and the lock they share
/// there. A scope used to make its keychain beside the files it signs, so
/// scopes on one host had nothing in common to coordinate on.
pub(super) fn signing_home() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context(
        "HOME is unset, so the signing scope has no $HOME/.stado/signing to keep its \
         temporary identity in",
    )?;
    let directory = PathBuf::from(home).join(".stado").join("signing");
    fs::create_dir_all(&directory)
        .with_context(|| format!("creating the signing directory {}", directory.display()))?;
    Ok(directory)
}

/// Wait for, then hold, the host's one signing lock. Every scope imports the
/// same fleet identity and Apple's intermediate into a keychain of its own,
/// lists it on the one user search list `codesign` builds chains from, and
/// deletes it when done; with two scopes open at once on one host (a
/// release worker's `macos-code-signing` and a product install, or two
/// builds) the second one's `codesign` fails `unable to build chain to
/// self-signed root` with `errSecInternalComponent`, while
/// `security find-identity -v` reads its identity as valid a moment later and
/// a scope signing alone passes. A waiting scope says whose turn it is
/// waiting for.
pub(super) fn hold_signing_lock(home: &Path) -> Result<fs::File> {
    crate::common::lock_waiting(&home.join("scope.lock")).context("taking the host's signing lock")
}

/// Keep Apple's public intermediates in one keychain of this host's signing
/// directory, `apple-issuers.keychain-db`, made once, never deleted, unlocked
/// and on the user's search list for every scope. Each scope used to import
/// the intermediate into its own temporary keychain and delete it with the
/// scope, and `codesign` then failed `unable to build chain to self-signed
/// root` on some signatures and not others, with no other scope open, while
/// a Mac whose login keychain holds the intermediates signs every time. The
/// keychain holds public certificates only, so it carries no password.
pub(in crate::signing) fn keep_apple_issuers(pem: &str) -> Result<()> {
    let home = signing_home()?;
    let keychain = home.join("apple-issuers.keychain-db");
    let path = keychain.to_str().context("non-UTF8 keychain path")?;
    if !keychain.exists() {
        command(
            "/usr/bin/security",
            &["create-keychain", "-p", "", path],
            true,
        )?;
        command("/usr/bin/security", &["set-keychain-settings", path], true)?;
    }
    command(
        "/usr/bin/security",
        &["unlock-keychain", "-p", "", path],
        true,
    )?;
    let issuers = home.join("apple-issuers.pem");
    atomic_write(&issuers, pem.as_bytes())?;
    let imported = command(
        "/usr/bin/security",
        &[
            "import",
            issuers.to_str().context("non-UTF8 issuer path")?,
            "-k",
            path,
        ],
        false,
    )?;
    let answer = String::from_utf8_lossy(&imported.stderr);
    if !imported.status.success() && !answer.contains("already exists") {
        bail!(
            "importing Apple's WWDR G3 intermediate into {path} failed ({}): {}",
            imported.status,
            answer.trim()
        );
    }
    let current = listed()?;
    if !current.iter().any(|listed| listed == path) {
        let mut arguments = vec!["list-keychains", "-d", "user", "-s"];
        arguments.extend(current.iter().map(String::as_str));
        arguments.push(path);
        command("/usr/bin/security", &arguments, true)?;
    }
    Ok(())
}
