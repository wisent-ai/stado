use super::{
    constants::{APPLE_ISSUERS_PEM, DEVELOPER_ID, DEVELOPMENT},
    core::{absolute, command, compatible, inspect, native, validate_identifier},
    credentials::Credentials,
    Policy,
};
use crate::common::sha256;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{fs, path::Path};

pub struct Signer {
    pub credentials: Credentials,
    requested: Option<String>,
}

impl Signer {
    pub fn new(requested: Option<&str>) -> Result<Self> {
        if !cfg!(target_os = "macos") {
            bail!("macOS code signing requires a Darwin host");
        }
        let requested = requested
            .map(str::to_owned)
            .or_else(|| std::env::var("WISENT_CODESIGN_IDENTITY").ok())
            .or_else(|| std::env::var("MACOS_SIGN_IDENTITY").ok());
        if requested.as_deref() == Some("-") {
            bail!("ad-hoc signing is not an installation identity");
        }
        let credentials = Credentials::open()?;
        trust_apple_issuers(&credentials)?;
        Ok(Self {
            credentials,
            requested,
        })
    }

    pub fn preserves_existing(&self, policy: &Policy) -> bool {
        self.requested.is_none() && policy.preserves_metadata()
    }

    pub fn identity(&self, previous: &Value) -> Result<String> {
        let mut arguments = vec!["find-identity", "-v", "-p", "codesigning"];
        let keychain = self
            .credentials
            .keychain
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned());
        if let Some(keychain) = keychain.as_ref() {
            arguments.push(keychain);
        }
        let listing = String::from_utf8(command("/usr/bin/security", &arguments, true)?.stdout)?;
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
                .find(|s| s.len() == 40 && s.bytes().all(|c| c.is_ascii_hexdigit()))
            else {
                continue;
            };
            if name.starts_with(DEVELOPER_ID) || name.starts_with(DEVELOPMENT) {
                certificates.push((digest, name));
            }
        }
        let selected = self
            .requested
            .as_deref()
            .or(self.credentials.identity.as_deref())
            .or_else(|| {
                if previous["state"] == "stable" {
                    previous["authority"].as_str()
                } else {
                    None
                }
            });
        if let Some(selected) = selected {
            let matches: Vec<_> = certificates
                .iter()
                .filter(|(digest, name)| selected == *name || selected.eq_ignore_ascii_case(digest))
                .collect();
            return match matches.as_slice() {
                [(digest, _)] => Ok((*digest).to_owned()),
                [] => {
                    let mut unfiltered = vec!["find-identity", "-p", "codesigning"];
                    if let Some(keychain) = keychain.as_ref() {
                        unfiltered.push(keychain);
                    }
                    let found = command("/usr/bin/security", &unfiltered, false)?;
                    bail!("Apple signing identity is missing or invalid: {selected}. Available valid identities: {certificates:?}. Unfiltered keychain result: {}. No different identity was selected.", String::from_utf8_lossy(&found.stdout));
                }
                _ => bail!("Apple signing identity is ambiguous: {selected}"),
            };
        }
        for prefix in [DEVELOPER_ID, DEVELOPMENT] {
            let matches: Vec<_> = certificates
                .iter()
                .filter(|(_, name)| name.starts_with(prefix))
                .collect();
            match matches.as_slice() {
                [(digest, _)] => return Ok((*digest).to_owned()),
                [] => {}
                _ => bail!(
                    "multiple Apple signing identities are available; set WISENT_CODESIGN_IDENTITY"
                ),
            }
        }
        bail!("no Apple signing identity is available; provision the product signing certificate before building")
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
        let signer = self.identity(&before)?;
        let original = sha256(&path)?;
        let staged = path.with_file_name(format!(".wisent-signing-{}", uuid::Uuid::new_v4()));
        fs::copy(&path, &staged)?;
        let result = (|| {
            let mut signed = self.stable_sign(&staged, &signer, identifier, policy)?;
            compatible(&staged, &before)?;
            if sha256(&path)? != original {
                bail!(
                    "signing target changed concurrently; no bytes replaced: {}",
                    path.display()
                );
            }
            fs::rename(&staged, &path)?;
            signed["path"] = json!(path);
            signed["previous_state"] = before["state"].clone();
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

/// Put Apple's intermediate into the keychain Stado made for supplied
/// material. A vault item holds the certificate alone, and a Mac whose
/// keychains never received the intermediate builds no chain for it:
/// `security find-identity -v` then lists no valid identity and the install
/// stops with `Apple signing identity is missing or invalid`. A keychain the
/// operator named (`WISENT_CODESIGN_KEYCHAIN`) is his own and left alone;
/// only a materialized identity, which names its digest, is completed.
fn trust_apple_issuers(credentials: &Credentials) -> Result<()> {
    let (Some(keychain), Some(_)) = (&credentials.keychain, &credentials.identity) else {
        return Ok(());
    };
    let directory = keychain
        .parent()
        .context("temporary signing keychain has no directory")?;
    let issuers = directory.join("apple-issuers.pem");
    fs::write(&issuers, APPLE_ISSUERS_PEM)
        .with_context(|| format!("writing Apple issuers to {}", issuers.display()))?;
    let found = command(
        "/usr/bin/security",
        &[
            "import",
            issuers.to_str().context("non-UTF8 issuer path")?,
            "-k",
            keychain.to_str().context("non-UTF8 keychain path")?,
        ],
        false,
    )?;
    // An intermediate the supplied chain already carried is a duplicate;
    // anything else is a failure the refusal must name.
    let answer = String::from_utf8_lossy(&found.stderr);
    if !found.status.success() && !answer.contains("already exists") {
        bail!(
            "importing Apple's WWDR G3 intermediate into {} failed ({}): {}",
            keychain.display(),
            found.status,
            answer.trim()
        );
    }
    Ok(())
}
