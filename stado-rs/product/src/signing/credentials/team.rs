//! The Apple team a signature will be anchored to, read before anything is
//! signed and without a private key: the subject of the certificate a PEM
//! pair or a role supplies, or the name of the identity a keychain holds.
//! A caller that must say in advance whether a program's code identity
//! changes reads it here, so the answer comes from the same source the
//! signature will come from.

use super::super::constants::{DEVELOPER_ID, DEVELOPMENT};
use super::super::core::command;
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::io::Write as _;
use std::process::{Command, Stdio};

/// Where the signing identity comes from, and the team it is anchored to.
#[derive(Clone, Debug, Serialize)]
pub struct Team {
    /// The certificate subject's organizational unit: the Apple team.
    pub team: String,
    /// The identity's name, as the certificate or keychain names it.
    pub identity: String,
    /// The source, in the words its refusals use.
    pub source: String,
    /// Whether `codesign` reads the key from a keychain of the operator's
    /// own (a named keychain, or the user's search list), where macOS may
    /// ask at this Mac for access to the key; a certificate in hand is put
    /// in a keychain the signing scope makes itself.
    pub operator_keychain: bool,
}

/// The team of the first certificate in `pem`: its subject's `OU`.
pub(super) fn certificate_team(pem: &str, source: &str) -> Result<Team> {
    let mut child = Command::new("/usr/bin/openssl")
        .args(["x509", "-noout", "-subject", "-nameopt", "RFC2253"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("/usr/bin/openssl did not start to read the signing certificate's subject")?;
    child
        .stdin
        .take()
        .context("openssl took no standard input")?
        .write_all(pem.as_bytes())
        .context("the signing certificate was not written to openssl")?;
    let output = child
        .wait_with_output()
        .context("openssl did not finish reading the signing certificate")?;
    if !output.status.success() {
        bail!(
            "the signing certificate from {source} cannot be read: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let subject = String::from_utf8(output.stdout)?;
    let subject = subject.trim().trim_start_matches("subject=").trim();
    let field = |name: &str| {
        subject
            .split(',')
            .filter_map(|part| part.trim().split_once('='))
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.replace('\\', ""))
    };
    let team = field("OU").with_context(|| {
        format!("the signing certificate from {source} names no team (no OU in {subject})")
    })?;
    let identity = match field("CN") {
        Some(name) => name,
        None => subject.to_string(),
    };
    Ok(Team {
        team,
        identity,
        source: source.to_string(),
        operator_keychain: false,
    })
}

/// The team of the identity `requested` names (by name or SHA-1) in
/// `keychain`, or the user's search list without one; with no identity
/// named, the keychain's only valid identity, else the search list's only
/// Developer ID, then its only Development identity — the choice the signer
/// itself makes.
pub(super) fn keychain_team(keychain: Option<&str>, requested: Option<&str>) -> Result<Team> {
    let mut arguments = vec!["find-identity", "-v", "-p", "codesigning"];
    if let Some(keychain) = keychain {
        arguments.push(keychain);
    }
    let searched = match keychain {
        Some(keychain) => format!("the keychain {keychain}"),
        None => "the user's keychain search list".to_owned(),
    };
    let listing = String::from_utf8(command("/usr/bin/security", &arguments, true)?.stdout)?;
    let certificates = super::selection::listed(&listing);
    let name = match requested {
        Some(selected) => {
            let matches: Vec<_> = certificates
                .iter()
                .filter(|(digest, name)| selected == *name || selected.eq_ignore_ascii_case(digest))
                .collect();
            match matches.as_slice() {
                [(_, name)] => (*name).to_owned(),
                [] => bail!(
                    "{searched} holds no valid Apple signing identity {selected}; available: \
                     {certificates:?}"
                ),
                _ => bail!("Apple signing identity {selected} is ambiguous in {searched}"),
            }
        }
        None if keychain.is_some() => match certificates.as_slice() {
            [(_, name)] => (*name).to_owned(),
            [] => bail!("{searched} holds no valid Apple signing identity"),
            several => bail!(
                "{searched} holds {} valid Apple signing identities {several:?}; name one with \
                 --signing-identity",
                several.len()
            ),
        },
        None => {
            let mut chosen = None;
            for prefix in [DEVELOPER_ID, DEVELOPMENT] {
                let matches: Vec<_> = certificates
                    .iter()
                    .filter(|(_, name)| name.starts_with(prefix))
                    .collect();
                match matches.as_slice() {
                    [(_, name)] => {
                        chosen = Some((*name).to_owned());
                        break;
                    }
                    [] => {}
                    _ => bail!(
                        "multiple Apple signing identities are available in {searched}; name \
                         one with --signing-identity"
                    ),
                }
            }
            chosen
                .with_context(|| format!("no Apple signing identity is available in {searched}"))?
        }
    };
    // An Apple identity's name ends in its team: `Developer ID Application:
    // Name (TEAM)`.
    let team = name
        .rsplit_once('(')
        .and_then(|(_, rest)| rest.strip_suffix(')'))
        .filter(|team| !team.is_empty())
        .with_context(|| format!("the signing identity {name} names no team in parentheses"))?
        .to_string();
    Ok(Team {
        team,
        identity: name,
        source: searched,
        operator_keychain: true,
    })
}
