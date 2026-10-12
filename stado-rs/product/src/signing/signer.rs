use super::{
    constants::APPLE_ISSUERS_PEM,
    core::{absolute, command, compatible, inspect, native, validate_identifier},
    credentials::{Choice, Credentials, Source},
    Policy,
};
use crate::common::sha256;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{fs, path::Path};

pub struct Signer {
    pub credentials: Credentials,
    /// The identity the caller or the operator named, by name or SHA-1.
    pub(super) requested: Option<String>,
}

impl Signer {
    /// A signer whose source is resolved now from the environment.
    pub fn new(requested: Option<&str>) -> Result<Self> {
        Self::chosen(&Choice::default(), requested)
    }

    /// A signer whose source is resolved now from `choice`.
    pub fn chosen(choice: &Choice, requested: Option<&str>) -> Result<Self> {
        Self::open(choice, requested, Source::resolve(choice, requested)?)
    }

    /// A signer over a source resolved earlier — before a build, so a source
    /// that cannot be read refused before the build started.
    pub fn open(choice: &Choice, requested: Option<&str>, source: Source) -> Result<Self> {
        if !cfg!(target_os = "macos") {
            bail!("macOS code signing requires a Darwin host");
        }
        let requested = choice
            .identity(requested)
            .or_else(|| std::env::var("MACOS_SIGN_IDENTITY").ok());
        if requested.as_deref() == Some("-") {
            bail!("ad-hoc signing is not an installation identity");
        }
        let credentials = Credentials::open(source)?;
        trust_apple_issuers(&credentials)?;
        Ok(Self {
            credentials,
            requested,
        })
    }

    pub fn preserves_existing(&self, policy: &Policy) -> bool {
        self.requested.is_none() && policy.preserves_metadata()
    }

    pub fn codesign(
        &self,
        path: &Path,
        identity: &str,
        identifier: &str,
        requirement: Option<&str>,
        policy: &Policy,
    ) -> Result<()> {
        let mut args = vec![
            "--force",
            "--sign",
            identity,
            "--identifier",
            identifier,
            "--timestamp=none",
        ];
        policy.arguments(&mut args)?;
        let keychain = self
            .credentials
            .keychain
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned());
        if let Some(keychain) = keychain.as_ref() {
            args.extend(["--keychain", keychain]);
        }
        let requirement = requirement.map(|value| format!("={value}"));
        if let Some(requirement) = requirement.as_ref() {
            args.extend(["--requirements", requirement]);
        }
        args.push(path.to_str().context("signing path is not UTF-8")?);
        self.credentials.unlock_for_signing()?;
        if let Err(error) = command("/usr/bin/codesign", &args, true) {
            return Err(match self.credentials.keychain_state() {
                Some(state) => error.context(format!("codesign {} with {state}", path.display())),
                None => error,
            });
        }
        Ok(())
    }

    pub fn stable_sign(
        &self,
        path: &Path,
        identity: &str,
        identifier: &str,
        policy: &Policy,
    ) -> Result<Value> {
        self.codesign(path, identity, identifier, None, policy)?;
        let first = inspect(path)?;
        if first["state"] != "stable" {
            bail!(
                "stable signing failed for {}: {}",
                path.display(),
                first["error"]
            );
        }
        let team = first["team"]
            .as_str()
            .context("signed code has no Apple team")?;
        let prepared = policy.prepare(path)?;
        let result = (|| {
            let policy = prepared.policy();
            let requirement = format!("designated => identifier \"{identifier}\" and anchor apple generic and certificate leaf[subject.OU] = \"{team}\"");
            self.codesign(path, identity, identifier, Some(&requirement), policy)?;
            let mut final_report = inspect(path)?;
            if final_report["state"] != "stable" {
                bail!("signed candidate is invalid: {}", final_report["error"]);
            }
            policy.verify(path, &mut final_report)?;
            Ok(final_report)
        })();
        prepared.finish(result)
    }

    pub fn sign(
        &self,
        path: &Path,
        identifier: &str,
        previous: Option<&Path>,
        policy: &Policy,
    ) -> Result<Value> {
        validate_identifier(identifier)?;
        let path = absolute(path)?.canonicalize()?;
        if path.is_dir() {
            return super::bundle::sign(self, &path, identifier, previous, policy);
        }
        if !native(&path)? {
            bail!("signing target is not a Mach-O file: {}", path.display());
        }
        let before = inspect(previous.filter(|p| p.exists()).unwrap_or(&path))?;
        if before["state"] == "stable" {
            if before["identifier"] != identifier {
                bail!(
                    "code identifier would change: {} -> {identifier}",
                    before["identifier"]
                );
            }
            if previous.is_none() && self.preserves_existing(policy) {
                return Ok(before);
            }
        }
        let candidate = inspect(&path)?;
        if self.preserves_existing(policy)
            && candidate["state"] == "stable"
            && candidate["identifier"] == identifier
            && compatible(&path, &before).is_ok()
        {
            return Ok(candidate);
        }
        let chosen = self.identity(&before)?;
        let original = sha256(&path)?;
        let staged = path.with_file_name(format!(".wisent-signing-{}", uuid::Uuid::new_v4()));
        fs::copy(&path, &staged)?;
        let result = (|| {
            let mut signed = self.stable_sign(&staged, &chosen.digest, identifier, policy)?;
            // The installed designated requirement binds its identity; a
            // change the operator chose replaces it, and is reported instead.
            if chosen.change.is_none() {
                compatible(&staged, &before)?;
            }
            if sha256(&path)? != original {
                bail!(
                    "signing target changed concurrently; no bytes replaced: {}",
                    path.display()
                );
            }
            fs::rename(&staged, &path)?;
            signed["path"] = json!(path);
            signed["previous_state"] = before["state"].clone();
            if let Some(change) = &chosen.change {
                signed["identity_change"] = change.clone();
            }
            Ok(signed)
        })();
        if staged.exists() {
            fs::remove_file(&staged)?;
        }
        result
    }

    pub fn close(&mut self) -> Result<()> {
        self.credentials.close()
    }
}

/// Make sure Apple's intermediate is where every signature on this host can
/// find it. A vault item holds the certificate alone, and a Mac whose
/// keychains never received the intermediate builds no chain for it:
/// `security find-identity -v` then lists no valid identity and the install
/// stops with `Apple signing identity is missing or invalid`. A keychain the
/// operator named (`WISENT_CODESIGN_KEYCHAIN`) is his own and left alone;
/// only a materialized identity, which names its digest, is completed.
fn trust_apple_issuers(credentials: &Credentials) -> Result<()> {
    let (Some(_), Some(_)) = (&credentials.keychain, &credentials.identity) else {
        return Ok(());
    };
    super::credentials::keep_apple_issuers(APPLE_ISSUERS_PEM)
}
