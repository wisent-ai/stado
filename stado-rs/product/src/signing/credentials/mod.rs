//! One signing scope: the identity it signs with, from the role's vault item,
//! a supplied PEM pair or the operator's own keychain. `keychain` keeps the
//! host's shared search list and Apple's issuers, `identity` makes the
//! scope's temporary keychain and reports on it when `codesign` fails.

mod identity;
mod keychain;

pub(super) use keychain::keep_apple_issuers;
use keychain::{blocks, put_first, take_off};

use super::core::command;
use anyhow::{bail, Context, Result};
use std::{fs, path::PathBuf};

pub struct Credentials {
    directory: Option<PathBuf>,
    pub keychain: Option<PathBuf>,
    pub identity: Option<String>,
    /// Whether this scope put its temporary keychain on the user's search
    /// list, so `close` takes exactly that one entry off again.
    listed: bool,
    temporary_keychain: bool,
    /// The host's signing lock, held from the moment this scope makes its
    /// keychain until `close` has deleted it.
    host_lock: Option<fs::File>,
}

/// The variable naming the operator's own keychain as the signing source.
const KEYCHAIN_VARIABLE: &str = "WISENT_CODESIGN_KEYCHAIN";

fn secret(role: &str, field: &str) -> Result<String> {
    crate::common::credential_field(role, field).with_context(|| {
        format!(
            "signing credential for role {role}, field {field}; no other identity was attempted. \
             To sign with an identity in your own keychain instead, set \
             {KEYCHAIN_VARIABLE}=<keychain path>: the role is then not read"
        )
    })
}

impl Credentials {
    pub fn open() -> Result<Self> {
        let mut certificate = std::env::var("WISENT_CODESIGN_CERTIFICATE_PEM")
            .ok()
            .filter(|s| !s.is_empty());
        let mut private_key = std::env::var("WISENT_CODESIGN_PRIVATE_KEY_PEM")
            .ok()
            .filter(|s| !s.is_empty());
        // The operator's own keychain is a signing source he names; naming it
        // beside a role or a PEM pair is two answers to one question.
        let keychain = std::env::var_os(KEYCHAIN_VARIABLE)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from);
        let named_role = std::env::var("WISENT_CODESIGN_ROLE").ok();
        if let Some(path) = &keychain {
            if named_role.is_some() || certificate.is_some() || private_key.is_some() {
                bail!(
                    "{KEYCHAIN_VARIABLE}={} names your own keychain as the signing source, and \
                     WISENT_CODESIGN_ROLE or the supplied PEM pair names another; name one",
                    path.display()
                );
            }
        }
        // Without a named source, Stado signs with the item that plays the
        // fleet's signing role; see [`crate::Build::signing_role`].
        let role = named_role.or_else(|| {
            let fleet = crate::build().signing_role;
            (certificate.is_none() && keychain.is_none() && !fleet.is_empty())
                .then(|| fleet.to_owned())
        });
        if let Some(role) = role {
            if role.trim().is_empty() || role.contains('#') {
                bail!("WISENT_CODESIGN_ROLE names a role, not a role#field coordinate");
            }
            if certificate.is_some() || private_key.is_some() {
                bail!("select either a signing role or the supplied PEM pair");
            }
            certificate = Some(secret(&role, "certificate")?);
            private_key = Some(secret(&role, "private_key")?);
        }
        if let Some(issuers) = std::env::var_os("WISENT_CODESIGN_ISSUERS_FILE") {
            if certificate.is_none() || private_key.is_none() {
                bail!("WISENT_CODESIGN_ISSUERS_FILE requires a supplied signing credential");
            }
            let issuers =
                fs::read_to_string(issuers).context("reading supplied signing issuers")?;
            blocks(&issuers)?;
            certificate
                .as_mut()
                .context("certificate missing")?
                .push_str(&format!("\n{issuers}"));
        }
        let mut scope = Self {
            directory: None,
            keychain: None,
            identity: None,
            listed: false,
            temporary_keychain: false,
            host_lock: None,
        };
        match (certificate, private_key) {
            (None, None) => {
                scope.keychain = keychain
                    .map(|path| {
                        path.canonicalize().with_context(|| {
                            format!("{KEYCHAIN_VARIABLE} names {}", path.display())
                        })
                    })
                    .transpose()?;
            }
            (Some(certificate), Some(private_key)) => {
                if !cfg!(target_os = "macos") {
                    bail!("supplied Apple signing credentials require a Darwin host");
                }
                scope.materialize(&certificate, &private_key)?;
            }
            _ => bail!(
                "provide both WISENT_CODESIGN_CERTIFICATE_PEM and WISENT_CODESIGN_PRIVATE_KEY_PEM"
            ),
        }
        Ok(scope)
    }

    /// Unlock the temporary keychain again and put it back first on the
    /// search list right before `codesign` reads its key. The keychain is
    /// unlocked and listed when it is made, but a release build signs
    /// minutes later in another process, and a keychain that locked, or that
    /// another signing job on the host took off the shared search list, in
    /// between fails the signature with `errSecInternalComponent` after the
    /// whole build has run. A keychain the operator named is his own and is
    /// never unlocked or listed here.
    pub fn unlock_for_signing(&self) -> Result<()> {
        if !self.temporary_keychain {
            return Ok(());
        }
        if let Some(keychain) = self.keychain.as_ref() {
            let keychain = keychain.to_str().context("non-UTF8 keychain path")?;
            command(
                "/usr/bin/security",
                &["unlock-keychain", "-p", "", keychain],
                true,
            )?;
            put_first(keychain)?;
        }
        Ok(())
    }

    pub fn close(&mut self) -> Result<()> {
        let mut errors = Vec::new();
        if std::mem::take(&mut self.listed) {
            if let Some(keychain) = self.keychain.as_ref() {
                if let Err(error) = take_off(&keychain.to_string_lossy()) {
                    errors.push(error.to_string());
                }
            }
        }
        if self.temporary_keychain {
            if let Some(keychain) = self.keychain.take() {
                if keychain.exists() {
                    if let Err(error) = command(
                        "/usr/bin/security",
                        &["delete-keychain", &keychain.to_string_lossy()],
                        true,
                    ) {
                        errors.push(error.to_string());
                    }
                }
            }
            self.temporary_keychain = false;
        }
        if let Some(directory) = self.directory.take() {
            if let Err(error) = fs::remove_dir_all(&directory) {
                errors.push(format!(
                    "removing signing material {}: {error}",
                    directory.display()
                ));
            }
        }
        // The next scope on this host starts only once this one's keychain
        // is off the search list and deleted.
        drop(self.host_lock.take());
        if !errors.is_empty() {
            bail!("signing cleanup failed: {}", errors.join("; "));
        }
        Ok(())
    }
}

impl Drop for Credentials {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            eprintln!("{error:#}");
        }
    }
}
