use super::{
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
    process::{Command, Stdio},
};

pub struct Credentials {
    directory: Option<PathBuf>,
    pub keychain: Option<PathBuf>,
    pub identity: Option<String>,
    search_list: Option<Vec<String>>,
    temporary_keychain: bool,
}

fn secret(item: &str, field: &str) -> Result<String> {
    // Secret stdout stays in memory; the general command recorder must not receive it.
    let result = Command::new("skarbiec")
        .args(["get", item, "--field", field])
        .stdin(Stdio::null())
        .output()?;
    if !result.status.success() {
        bail!("signing credential {item}#{field}: Skarbiec exited {}; no other identity was attempted", result.status);
    }
    let value = String::from_utf8(result.stdout)?;
    if value.trim().is_empty() {
        bail!("signing credential {item}#{field}: Skarbiec returned no value");
    }
    Ok(value)
}

fn blocks(text: &str) -> Result<Vec<String>> {
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

impl Credentials {
    pub fn open(root: &Path) -> Result<Self> {
        let mut certificate = std::env::var("WISENT_CODESIGN_CERTIFICATE_PEM")
            .ok()
            .filter(|s| !s.is_empty());
        let mut private_key = std::env::var("WISENT_CODESIGN_PRIVATE_KEY_PEM")
            .ok()
            .filter(|s| !s.is_empty());
        // Without a named credential, Stado signs with the fleet's item; see
        // [`crate::Build::signing_item`].
        let item = std::env::var("WISENT_CODESIGN_CREDENTIAL_ITEM")
            .ok()
            .or_else(|| {
                let fleet = crate::build().signing_item;
                (certificate.is_none() && !fleet.is_empty()).then(|| fleet.to_owned())
            });
        if let Some(item) = item {
            if item.trim().is_empty() || item.contains('#') {
                bail!("WISENT_CODESIGN_CREDENTIAL_ITEM requires an item id, not an item#field coordinate");
            }
            if certificate.is_some() || private_key.is_some() {
                bail!("select either a signing credential item or the supplied PEM pair");
            }
            certificate = Some(secret(&item, "certificate")?);
            private_key = Some(secret(&item, "private_key")?);
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
            search_list: None,
            temporary_keychain: false,
        };
        match (certificate, private_key) {
            (None, None) => {
                scope.keychain = std::env::var_os("WISENT_CODESIGN_KEYCHAIN")
                    .map(PathBuf::from)
                    .map(|p| p.canonicalize())
                    .transpose()?;
            }
            (Some(certificate), Some(private_key)) => {
                if !cfg!(target_os = "macos") {
                    bail!("supplied Apple signing credentials require a Darwin host");
                }
                scope.materialize(root, &certificate, &private_key)?;
            }
            _ => bail!(
                "provide both WISENT_CODESIGN_CERTIFICATE_PEM and WISENT_CODESIGN_PRIVATE_KEY_PEM"
            ),
        }
        Ok(scope)
    }

    fn materialize(&mut self, root: &Path, certificate: &str, private_key: &str) -> Result<()> {
        let chain = blocks(certificate)?;
        let directory = root.join(format!(".wisent-identity-{}", uuid::Uuid::new_v4()));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&directory)?;
        self.directory = Some(directory.clone());
        let certificate_path = directory.join("certificate.pem");
        let key_path = directory.join("private-key.pem");
        atomic_write(&certificate_path, chain[0].as_bytes())?;
        atomic_write(&key_path, private_key.as_bytes())?;
        let cert = certificate_path
            .to_str()
            .context("non-UTF8 credential path")?;
        let key = key_path.to_str().context("non-UTF8 credential path")?;
        let cert_public = command(
            "/usr/bin/openssl",
            &["x509", "-in", cert, "-pubkey", "-noout"],
            true,
        )?;
        let key_public = command("/usr/bin/openssl", &["pkey", "-in", key, "-pubout"], true)?;
        if cert_public.stdout != key_public.stdout {
            bail!("signing certificate and private key do not match");
        }
        let fingerprint = String::from_utf8(
            command(
                "/usr/bin/openssl",
                &["x509", "-in", cert, "-fingerprint", "-sha1", "-noout"],
                true,
            )?
            .stdout,
        )?;
        self.identity = Some(
            fingerprint
                .trim()
                .split_once('=')
                .context("certificate fingerprint missing")?
                .1
                .replace(':', ""),
        );
        let keychain = directory.join("identity.keychain-db");
        self.keychain = Some(keychain.clone());
        self.temporary_keychain = true;
        let keychain = keychain.to_str().context("non-UTF8 keychain path")?;
        // Empty transport passwords belong only to this private temporary keychain.
        command(
            "/usr/bin/security",
            &["create-keychain", "-p", "", keychain],
            true,
        )?;
        command(
            "/usr/bin/security",
            &["set-keychain-settings", keychain],
            true,
        )?;
        command(
            "/usr/bin/security",
            &["unlock-keychain", "-p", "", keychain],
            true,
        )?;
        for (index, issuer) in chain.iter().enumerate().skip(1) {
            let path = directory.join(format!("issuer-{index}.pem"));
            atomic_write(&path, issuer.as_bytes())?;
            command(
                "/usr/bin/security",
                &[
                    "import",
                    path.to_str().context("non-UTF8 issuer path")?,
                    "-k",
                    keychain,
                ],
                true,
            )?;
        }
        command("/usr/bin/security", &["import", cert, "-k", keychain], true)?;
        command(
            "/usr/bin/security",
            &[
                "import",
                key,
                "-k",
                keychain,
                "-P",
                "",
                "-T",
                "/usr/bin/codesign",
            ],
            true,
        )?;
        command(
            "/usr/bin/security",
            &[
                "set-key-partition-list",
                "-S",
                "apple-tool:,apple:,codesign:",
                "-s",
                "-k",
                "",
                keychain,
            ],
            true,
        )?;
        let previous = listed()?;
        self.search_list = Some(previous.clone());
        let mut arguments = vec!["list-keychains", "-d", "user", "-s", keychain];
        arguments.extend(previous.iter().map(String::as_str));
        command("/usr/bin/security", &arguments, true)?;
        Ok(())
    }

    pub fn close(&mut self) -> Result<()> {
        let mut errors = Vec::new();
        if let Some(previous) = self.search_list.take() {
            let mut arguments = vec!["list-keychains", "-d", "user", "-s"];
            arguments.extend(previous.iter().map(String::as_str));
            if let Err(error) = command("/usr/bin/security", &arguments, true) {
                errors.push(error.to_string());
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
