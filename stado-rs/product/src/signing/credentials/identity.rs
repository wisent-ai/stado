//! The scope's own identity: a temporary keychain holding the supplied
//! certificate and key, and what the keychain answers when `codesign` fails.

use super::super::core::command;
use super::keychain::{blocks, hold_signing_lock, keep_apple_issuers, put_first, signing_home};
use super::Credentials;
use crate::common::atomic_write;
use anyhow::{bail, Context, Result};
use std::fs;

impl Credentials {
    pub(super) fn materialize(&mut self, certificate: &str, private_key: &str) -> Result<()> {
        let chain = blocks(certificate)?;
        let home = signing_home()?;
        self.host_lock = Some(hold_signing_lock(&home)?);
        let directory = home.join(format!(".wisent-identity-{}", uuid::Uuid::new_v4()));
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
        // Issuers that came with the certificate go into the persistent
        // issuers keychain, never into this one. A temporary keychain holding
        // Apple's intermediate and then deleted left the next scope's
        // `codesign` failing `unable to build chain to self-signed root` /
        // `errSecInternalComponent` while `verify-cert` and
        // `find-identity -v` passed: the release worker signs with the
        // certificate and its chain from its environment, and a product
        // install signing right after it failed every time, while the same
        // install alone, or two signatures with no chain supplied, signed.
        let issuers: String = chain.iter().skip(1).cloned().collect();
        if !issuers.is_empty() {
            keep_apple_issuers(&issuers)?;
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
        put_first(keychain)?;
        self.listed = true;
        Ok(())
    }

    /// The signing keychain and what each read answers at the moment
    /// `codesign` failed — each exit status and its own words, not a reading
    /// of them: `security show-keychain-info` (whether the keychain is there
    /// and unlocked), `security find-identity -v -p codesigning` on it
    /// (whether its identity validates), the user's keychain search list
    /// (whether `codesign` searches it at all), `security verify-cert -p
    /// codeSign` on the leaf certificate (the chain evaluation itself, with
    /// the error it stops on), every Apple Worldwide Developer Relations
    /// certificate the search list holds with its SHA-1 and keychain (a stale
    /// or second intermediate is one way the chain fails), and the user's
    /// trust settings (a certificate marked untrusted is another).
    /// `find-identity -v` can read the identity as valid right after
    /// `codesign` failed `unable to build chain to self-signed root`, so the
    /// first three alone do not tell the cause.
    pub fn keychain_state(&self) -> Option<String> {
        let keychain = self.keychain.as_ref()?.to_string_lossy().into_owned();
        let leaf = self.directory.as_ref().map(|directory| {
            directory
                .join("certificate.pem")
                .to_string_lossy()
                .into_owned()
        });
        let mut reads = vec![
            vec!["show-keychain-info", keychain.as_str()],
            vec![
                "find-identity",
                "-v",
                "-p",
                "codesigning",
                keychain.as_str(),
            ],
            vec!["list-keychains", "-d", "user"],
            vec![
                "find-certificate",
                "-a",
                "-c",
                "Apple Worldwide Developer Relations",
                "-Z",
            ],
            vec!["dump-trust-settings"],
        ];
        if let Some(leaf) = leaf.as_deref() {
            reads.push(vec!["verify-cert", "-c", leaf, "-p", "codeSign"]);
        }
        let observed = reads
            .iter()
            .map(|arguments| {
                let spelled = arguments.join(" ");
                match command("/usr/bin/security", arguments, false) {
                    Ok(output) => format!(
                        "security {spelled} exited {}: {} {}",
                        output.status,
                        String::from_utf8_lossy(&output.stdout).trim(),
                        String::from_utf8_lossy(&output.stderr).trim()
                    ),
                    Err(error) => format!("security {spelled} could not run: {error:#}"),
                }
            })
            .collect::<Vec<String>>()
            .join("; ");
        Some(format!("signing keychain {keychain} ({observed})"))
    }
}
