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
};

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

fn secret(role: &str, field: &str) -> Result<String> {
    crate::common::credential_field(role, field).with_context(|| {
        format!(
            "signing credential for role {role}, field {field}; no other identity was attempted"
        )
    })
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

/// Put `keychain` first on the user's keychain search list, reading the list
/// as it is now. Signing jobs on one host share that list: a scope that
/// restored a snapshot taken when it opened dropped every keychain another
/// job had added since, and that job's `codesign` then built no chain for
/// its identity and failed with `errSecInternalComponent` (55167f6e). The
/// same restores left entries naming temporary keychains already deleted;
/// an entry whose file is gone names nothing to search and is not kept.
fn put_first(keychain: &str) -> Result<()> {
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
fn take_off(keychain: &str) -> Result<()> {
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
fn signing_home() -> Result<PathBuf> {
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
/// deletes it when done; two scopes open at once on charless-mac-mini (a
/// release worker's `macos-code-signing` and a product install, or two
/// builds) left the second one's `codesign` failing `unable to build chain
/// to self-signed root` with `errSecInternalComponent`, while
/// `security find-identity -v` read its identity as valid a moment later and
/// a scope signing alone passed (55167f6e). A waiting scope says whose turn
/// it is waiting for.
fn hold_signing_lock(home: &Path) -> Result<fs::File> {
    crate::common::lock_waiting(&home.join("scope.lock")).context("taking the host's signing lock")
}

/// Keep Apple's public intermediates in one keychain of this host's signing
/// directory, `apple-issuers.keychain-db`, made once, never deleted, unlocked
/// and on the user's search list for every scope. Each scope used to import
/// the intermediate into its own temporary keychain and delete it with the
/// scope; on charless-mac-mini `codesign` then failed `unable to build chain
/// to self-signed root` on some signatures and not others, with no other
/// scope open, while a Mac whose login keychain holds the intermediates
/// signs every time (55167f6e). The keychain holds public certificates only,
/// so it carries no password.
pub(super) fn keep_apple_issuers(pem: &str) -> Result<()> {
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
impl Credentials {
    pub fn open() -> Result<Self> {
        let mut certificate = std::env::var("WISENT_CODESIGN_CERTIFICATE_PEM")
            .ok()
            .filter(|s| !s.is_empty());
        let mut private_key = std::env::var("WISENT_CODESIGN_PRIVATE_KEY_PEM")
            .ok()
            .filter(|s| !s.is_empty());
        // Without supplied PEM material, Stado signs with the item that plays
        // the signing role; see [`crate::Build::signing_role`].
        let role = std::env::var("WISENT_CODESIGN_ROLE").ok().or_else(|| {
            let fleet = crate::build().signing_role;
            (certificate.is_none() && !fleet.is_empty()).then(|| fleet.to_owned())
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
                scope.keychain = std::env::var_os("WISENT_CODESIGN_KEYCHAIN")
                    .map(PathBuf::from)
                    .map(|p| p.canonicalize())
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

    fn materialize(&mut self, certificate: &str, private_key: &str) -> Result<()> {
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
        // `codesign` on charless-mac-mini failing `unable to build chain to
        // self-signed root` / `errSecInternalComponent` while `verify-cert`
        // and `find-identity -v` passed: the release worker signs with the
        // certificate and its chain from its environment, and the post-build
        // product install that signed seconds after it failed every time,
        // while the same install alone, or two signatures with no chain
        // supplied, signed (55167f6e).
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
    /// trust settings (a certificate marked untrusted is another). On
    /// charless-mac-mini `find-identity -v` read the identity as valid right
    /// after `codesign` failed `unable to build chain to self-signed root`
    /// (55167f6e), so the first three alone do not tell the cause.
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
