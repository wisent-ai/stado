//! Which identity a signature is made with, chosen from the scope's keychain
//! (or the user's search list), and whether choosing it changes the identity
//! the installed code is signed by.

use super::super::{
    constants::{DEVELOPER_ID, DEVELOPMENT},
    core::command,
    signer::Signer,
};
use super::{IDENTITY_VARIABLE, KEYCHAIN_VARIABLE};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

/// The identity a signature is made with, and, when it is not the identity
/// the installed code is signed by, the change (`previous` -> `new`) the
/// operator chose by naming it.
pub struct Chosen {
    pub digest: String,
    pub change: Option<Value>,
}

/// The identities `security find-identity -v -p codesigning` lists, as
/// (SHA-1, name): each line is `  <n>) <SHA-1> "<name>"`, so the digest is the
/// last word before the quoted name.
fn listed(listing: &str) -> Vec<(&str, &str)> {
    let mut certificates = Vec::new();
    for line in listing.lines() {
        let Some((prefix, rest)) = line.split_once('"') else {
            continue;
        };
        let Some((name, _)) = rest.split_once('"') else {
            continue;
        };
        let Some(digest) = prefix
            .split_whitespace()
            .last()
            .filter(|word| word.bytes().all(|c| c.is_ascii_hexdigit()))
        else {
            continue;
        };
        if name.starts_with(DEVELOPER_ID) || name.starts_with(DEVELOPMENT) {
            certificates.push((digest, name));
        }
    }
    certificates
}

impl Signer {
    /// Choose the identity from the scope's keychain (or the search list):
    /// the one the caller named, else the one the scope's material carries,
    /// else the authority the installed code is signed by, else the only
    /// valid identity there. Signing with another identity than the installed
    /// stable one is the operator's explicit choice: allowed only when he
    /// named the new identity, and reported as `identity_change`.
    pub fn identity(&self, previous: &Value) -> Result<Chosen> {
        let mut arguments = vec!["find-identity", "-v", "-p", "codesigning"];
        let keychain = self
            .credentials
            .keychain
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned());
        if let Some(keychain) = keychain.as_ref() {
            arguments.push(keychain);
        }
        let searched = match keychain.as_deref() {
            Some(keychain) => format!("the keychain {keychain}"),
            None => "the user's keychain search list".to_owned(),
        };
        let listing = String::from_utf8(command("/usr/bin/security", &arguments, true)?.stdout)?;
        let certificates = listed(&listing);
        let installed = if previous["state"] == "stable" {
            Some(
                previous["authority"]
                    .as_str()
                    .context("the installed code's stable signature names no authority")?,
            )
        } else {
            None
        };
        let named = self
            .requested
            .as_deref()
            .or(self.credentials.identity.as_deref());
        let (digest, name) = match named.or(installed) {
            Some(selected) => {
                let matches: Vec<_> = certificates
                    .iter()
                    .filter(|(digest, name)| {
                        selected == *name || selected.eq_ignore_ascii_case(digest)
                    })
                    .collect();
                match matches.as_slice() {
                    [(digest, name)] => ((*digest).to_owned(), (*name).to_owned()),
                    [] if named.is_none() => bail!(
                        "the installed code is signed by {selected}, which {searched} does not \
                         hold as a valid identity; available: {certificates:?}. Signing with \
                         another identity changes the installed one, which only the operator \
                         chooses: name the keychain with {KEYCHAIN_VARIABLE} (--signing-keychain) \
                         and the new identity, by its name or SHA-1, with {IDENTITY_VARIABLE} \
                         (--signing-identity); the installation report then names the change"
                    ),
                    [] => {
                        let mut unfiltered = vec!["find-identity", "-p", "codesigning"];
                        if let Some(keychain) = keychain.as_ref() {
                            unfiltered.push(keychain);
                        }
                        let found = command("/usr/bin/security", &unfiltered, false)?;
                        bail!("Apple signing identity is missing or invalid: {selected}. Available valid identities in {searched}: {certificates:?}. Unfiltered keychain result: {}. No different identity was selected.", String::from_utf8_lossy(&found.stdout));
                    }
                    _ => bail!("Apple signing identity is ambiguous in {searched}: {selected}"),
                }
            }
            None => self.sole(&certificates, &searched)?,
        };
        let change = match installed {
            Some(previous) if previous != name => {
                if self.requested.is_none() {
                    bail!(
                        "signing would change the installed identity {previous} to {name}; a \
                         change of the stable identity is the operator's explicit choice: name \
                         the new identity with {IDENTITY_VARIABLE} (--signing-identity)"
                    );
                }
                Some(json!({"previous": previous, "new": name}))
            }
            _ => None,
        };
        Ok(Chosen { digest, change })
    }

    /// The identity when none was named and nothing is installed: in a
    /// keychain the operator named, its only valid identity, refused when it
    /// holds several; elsewhere the only Developer ID, then the only
    /// Development identity.
    fn sole(&self, certificates: &[(&str, &str)], searched: &str) -> Result<(String, String)> {
        let owned = |(digest, name): &(&str, &str)| ((*digest).to_owned(), (*name).to_owned());
        if self.credentials.operator_keychain {
            return match certificates {
                [only] => Ok(owned(only)),
                [] => bail!("{searched} holds no valid Apple signing identity"),
                several => bail!(
                    "{searched} holds {} valid Apple signing identities {several:?}; name one by \
                     its name or SHA-1 with {IDENTITY_VARIABLE} (--signing-identity)",
                    several.len()
                ),
            };
        }
        for prefix in [DEVELOPER_ID, DEVELOPMENT] {
            let matches: Vec<_> = certificates
                .iter()
                .filter(|(_, name)| name.starts_with(prefix))
                .collect();
            match matches.as_slice() {
                [only] => return Ok(owned(*only)),
                [] => {}
                _ => bail!(
                    "multiple Apple signing identities are available in {searched}; set \
                     {IDENTITY_VARIABLE} (--signing-identity)"
                ),
            }
        }
        bail!("no Apple signing identity is available in {searched}; provision the product signing certificate before building")
    }
}
