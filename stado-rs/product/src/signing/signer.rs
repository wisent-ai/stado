use super::{
    constants::{DEVELOPER_ID, DEVELOPMENT},
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
    pub fn new(root: &Path, requested: Option<&str>) -> Result<Self> {
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
        Ok(Self {
            credentials: Credentials::open(root)?,
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
        command("/usr/bin/codesign", &args, true)?;
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
