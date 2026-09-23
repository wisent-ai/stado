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

pub(super) fn verify(home: &Path, host: &str, presented: &PublicKeyOrCertificate) -> Result<()> {
    let public_key = presented.public_key();
    let mut accepted = false;
    for path in [
        home.join(".ssh/known_hosts"),
        home.join(".ssh/known_hosts2"),
        "/etc/ssh/ssh_known_hosts".into(),
        "/etc/ssh/ssh_known_hosts2".into(),
    ] {
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
