//! Resolve one explicit key selector and prove that key's decryption.

use super::remote::VaultHost;
use crate::deploy::Runner;

pub(super) async fn prove(
    destination: &VaultHost,
    selector: &str,
    runner: &Runner,
) -> Result<String, String> {
    let listing = destination.gpg(
        &["--with-colons", "--with-fingerprint", "--list-secret-keys", "--", selector],
        None,
        runner,
    ).await?;
    let mut primary = false;
    let mut selected = None;
    for line in listing.lines() {
        match line.split(':').next() {
            Some("sec") => primary = true,
            Some("ssb") => primary = false,
            Some("fpr") if primary => {
                primary = false;
                // GnuPG doc/DETAILS and Skarbiec's crypto::keys read the hex
                // payload of an fpr record. Refuse additional populated fields
                // rather than confusing future metadata with an identity.
                let mut fields = line.strip_prefix("fpr:").unwrap()
                    .split(':').filter(|field| !field.is_empty());
                let fingerprint = fields.next().filter(|value| {
                    value.bytes().all(|byte| byte.is_ascii_hexdigit())
                }).ok_or_else(|| format!(
                    "{}: secret-key listing for {selector} returned an invalid fingerprint record: {line}",
                    destination.target.name,
                ))?;
                if fields.next().is_some() {
                    return Err(format!(
                        "{}: secret-key listing for {selector} returned an unsupported fingerprint record: {line}",
                        destination.target.name,
                    ));
                }
                if let Some(previous) = selected {
                    return Err(format!(
                        "{}: identity {selector} selects multiple secret keys, {previous} and {fingerprint}; name one exact fingerprint before registering an owner",
                        destination.target.name,
                    ));
                }
                selected = Some(fingerprint);
            }
            _ => {}
        }
    }
    let fingerprint = selected.ok_or_else(|| format!(
        "{}: secret-key listing returned no primary fingerprint for {selector}",
        destination.target.name,
    ))?;
    // A listing can contain a stub or an unavailable hardware key. Prove the
    // selected certificate's actual encryption/decryption, without vault writes
    // or exporting any secret key material.
    let challenge = uuid::Uuid::new_v4().to_string();
    let encrypted = destination.gpg(
        &["--armor", "--trust-model", "always", "--recipient", fingerprint, "--encrypt"],
        Some(&challenge),
        runner,
    ).await?;
    let opened = destination.gpg(&["--decrypt"], Some(&encrypted), runner).await?;
    if opened != challenge {
        return Err(format!(
            "{}: secret key {fingerprint} did not return the exact readiness challenge",
            destination.target.name,
        ));
    }
    Ok(fingerprint.to_string())
}
