//! The process the vault says holds its key database's lock, read off a
//! verifier's failed answer.
//!
//! A vault whose gpg waited on the keyring lock and gave up answers 503 with
//! gpg's own `waiting for lock (held by <pid>)` and, from a Skarbiec that
//! reads its lock files, `<lock> is held by pid <pid> on <host> (<what it
//! runs>), taken <when>`. Neither is a status code: the holder is known only
//! from those words, which is why this is the one place in Stado that reads
//! them, so a held lock is recognised the same way by the object API's
//! refusals and its repair, the doctor and the crypto repair payload
//! (`host_payloads/recover-skarbiec-crypto.sh`), which admits a recovery on
//! the same two phrases.

use super::super::SkarbiecError;

/// The phrases a held keyring lock is named by: gpg's own note, and
/// Skarbiec's lock-file reading.
pub const KEYRING_LOCK_PHRASES: &[&str] = &["waiting for lock (held by ", " is held by pid "];

/// The part of `detail` from the first phrase naming a held keyring lock,
/// one line, or `None` when it names no lock holder. The vault's words reach
/// the reader unchanged, so the pid, host and command are the vault's own.
pub fn keyring_lock_sentence(detail: &str) -> Option<String> {
    let at = KEYRING_LOCK_PHRASES
        .iter()
        .filter_map(|phrase| detail.find(phrase))
        .min()?;
    let sentence = detail[at..].trim_start();
    let line = match sentence.split_once('\n') {
        Some((line, _)) => line,
        None => sentence,
    };
    Some(line.trim().to_string())
}

impl SkarbiecError {
    /// The sentence in which the vault names the process holding its key
    /// database's lock, when its answer carries one; `None` otherwise.
    pub fn keyring_lock_holder(&self) -> Option<String> {
        match self {
            Self::Response { detail, .. } => keyring_lock_sentence(detail),
            Self::Read { source, .. } => source.keyring_lock_holder(),
            _ => None,
        }
    }
}
