//! Keep OpenSSH's host-key pinning, revocations and certificate authority checks.

use std::path::Path;

use anyhow::{bail, ensure, Context, Result};
use ring::hmac;
use russh::keys::{
    ssh_key::{
        certificate::CertType,
        known_hosts::{HostPatterns, KnownHosts, Marker},
    },
    HashAlg, PublicKeyOrCertificate,
};

fn matches(patterns: &HostPatterns, host: &str) -> Result<bool> {
    match patterns {
        HostPatterns::HashedName { salt, hash } => {
            let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, salt);
            Ok(hmac::verify(&key, host.as_bytes(), hash).is_ok())
        }
        HostPatterns::Patterns(patterns) => {
            let mut accepted = false;
            for pattern in patterns {
                let (negative, pattern) = match pattern.strip_prefix('!') {
                    Some(pattern) => (true, pattern),
                    None => (false, pattern.as_str()),
                };
                let expression = regex::escape(pattern)
                    .replace("\\*", ".*")
                    .replace("\\?", ".");
                if regex::RegexBuilder::new(&format!("^{expression}$"))
                    .case_insensitive(true)
                    .build()?
                    .is_match(host)
                {
                    if negative {
                        return Ok(false);
                    }
                    accepted = true;
                }
            }
            Ok(accepted)
        }
    }
}

/// The files OpenSSH reads host keys from, the user's first.
fn known_hosts_files(home: &Path) -> [std::path::PathBuf; 4] {
    [
        home.join(".ssh/known_hosts"),
        home.join(".ssh/known_hosts2"),
        "/etc/ssh/ssh_known_hosts".into(),
        "/etc/ssh/ssh_known_hosts2".into(),
    ]
}

/// [`verify`] for a connection whose transport already proved the machine
/// (Stado's network admits a node only by the key the registry declares for
/// its host): a host key never recorded under `host` is learned into the
/// user's known_hosts. A name that has an entry keeps OpenSSH's rules, so a
/// changed or revoked key is still refused.
pub(super) fn verify_or_learn(
    home: &Path,
    host: &str,
    presented: &PublicKeyOrCertificate,
) -> Result<()> {
    let refused = match verify(home, host, presented) {
        Ok(()) => return Ok(()),
        Err(refused) => refused,
    };
    for path in known_hosts_files(home) {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
        };
        for entry in KnownHosts::new(&text) {
            let entry = entry.with_context(|| format!("parse {}", path.display()))?;
            if matches(entry.host_patterns(), host)? {
                return Err(refused.context(format!(
                    "{} already records {host}, so its key is not learned over the Stado network",
                    path.display()
                )));
            }
        }
    }
    ensure!(
        presented.certificate().is_none(),
        "SSH host {host} presents a certificate no recorded authority signs; certificates are not learned"
    );
    let line = presented
        .public_key()
        .to_openssh()
        .context("encode the SSH host key")?;
    let path = home.join(".ssh/known_hosts");
    if let Some(directory) = path.parent() {
        std::fs::create_dir_all(directory)
            .with_context(|| format!("create {}", directory.display()))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    std::io::Write::write_all(&mut file, format!("{host} {line}\n").as_bytes())
        .with_context(|| format!("record {host}'s host key in {}", path.display()))
}

pub(super) fn verify(home: &Path, host: &str, presented: &PublicKeyOrCertificate) -> Result<()> {
    let public_key = presented.public_key();
    let mut accepted = false;
    for path in known_hosts_files(home) {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
        };
        for entry in KnownHosts::new(&text) {
            let entry = entry.with_context(|| format!("parse {}", path.display()))?;
            if !matches(entry.host_patterns(), host)? {
                continue;
            }
            let same_key = entry.public_key().key_data() == public_key.key_data();
            let signing_key = presented.certificate().is_some_and(|certificate| {
                certificate.signature_key() == entry.public_key().key_data()
            });
            match entry.marker() {
                Some(Marker::Revoked) if same_key || signing_key => {
                    bail!(
                        "SSH host {host} presents a key revoked in {}",
                        path.display()
                    );
                }
                Some(Marker::CertAuthority) if signing_key => {
                    let certificate = presented
                        .certificate()
                        .context("host certificate is absent")?;
                    ensure!(
                        certificate.cert_type() == CertType::Host,
                        "SSH certificate for {host} is not a host certificate"
                    );
                    ensure!(
                        certificate
                            .valid_principals()
                            .iter()
                            .any(|principal| principal.eq_ignore_ascii_case(host)),
                        "SSH certificate does not authorize host {host}"
                    );
                    ensure!(
                        certificate.critical_options().is_empty(),
                        "SSH certificate for {host} has unsupported critical options"
                    );
                    certificate
                        .validate(&[entry.public_key().fingerprint(HashAlg::Sha256)])
                        .with_context(|| format!("validate SSH host certificate for {host}"))?;
                    accepted = true;
                }
                None if same_key && presented.certificate().is_none() => accepted = true,
                _ => {}
            }
        }
    }
    ensure!(accepted, "SSH host {host} has no matching trusted key; unknown or changed keys are refused, never learned automatically");
    Ok(())
}
