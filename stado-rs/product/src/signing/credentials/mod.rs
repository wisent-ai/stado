//! One signing scope: the identity it signs with, from the role's vault item,
//! a supplied PEM pair or the operator's own keychain, chosen by [`Source`]
//! before the build. `keychain` keeps the host's shared search list and
//! Apple's issuers, `identity` makes the scope's temporary keychain and
//! reports on it when `codesign` fails.

mod identity;
mod keychain;
mod source;

pub(super) use keychain::keep_apple_issuers;
use keychain::{blocks, put_first, take_off};
pub use source::{Choice, Source, IDENTITY_VARIABLE, KEYCHAIN_VARIABLE};

mod selection;

use super::core::command;
use anyhow::{bail, Context, Result};
use std::{fs, path::PathBuf};

pub struct Credentials {
    directory: Option<PathBuf>,
    pub keychain: Option<PathBuf>,
    pub identity: Option<String>,
    /// Whether the keychain is one the operator named, whose identity he
    /// chooses: by name or digest, or as its only valid identity.
    pub operator_keychain: bool,
    /// Whether this scope put its temporary keychain on the user's search
    /// list, so `close` takes exactly that one entry off again.
    listed: bool,
    temporary_keychain: bool,
    /// The host's signing lock, held from the moment this scope makes its
    /// keychain until `close` has deleted it.
    host_lock: Option<fs::File>,
}

fn secret(role: &str, field: &str) -> Result<String> {
    crate::common::credential_field(role, field).with_context(|| {
        format!(
            "signing credential for role {role}, field {field}; no other identity was attempted"
        )
    })
}

impl Credentials {
    /// Open the scope `source` names. A role was already read when the
    /// source was resolved; a certificate in hand becomes a temporary
    /// keychain only now, so the host's signing lock is held for the
    /// signature, not for the build before it.
    pub fn open(source: Source) -> Result<Self> {
        let mut scope = Self {
            directory: None,
            keychain: None,
            identity: None,
            operator_keychain: source.operator_keychain(),
            listed: false,
            temporary_keychain: false,
            host_lock: None,
        };
        match source {
            Source::SearchList => {}
            Source::Keychain(keychain) => scope.keychain = Some(keychain),
            Source::Material {
                certificate,
                private_key,
                from,
            } => {
                if !cfg!(target_os = "macos") {
                    bail!("Apple signing credentials from {from} require a Darwin host");
                }
                scope
                    .materialize(&certificate, &private_key)
                    .with_context(|| format!("signing credential from {from}"))?;
            }
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
