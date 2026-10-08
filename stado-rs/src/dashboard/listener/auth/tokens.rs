//! The namespace bearer cache and the release publisher bearer read.
//!
//! Object traffic must not turn into one value read per object request: a
//! value read decrypts the item and appends an audit entry in the vault, and
//! one per request exhausted it and made the object plane answer 503. A
//! namespace bearer is therefore held with the version it was read under
//! (Skarbiec's item, item_uid and revision), and every request asks the vault
//! only for the item's current version (`/v1/items/revision`, which decrypts
//! nothing): the same version uses the held bearer, a different one reads the
//! bearer again. A rotated or replaced bearer stops working on the next
//! request. It used to be held for 60 seconds (OBJECT_TOKEN_FRESH_FOR, a
//! window nobody stated), so a rotated bearer kept authorizing writes for up
//! to a minute.
//!
//! A vault read that fails is answered as a failure (a redacted 503), never
//! with a bearer held from earlier. A store that keeps no versions (the file
//! backend) has its bearer read on every request.

use crate::dashboard::listener::Dashboard;
use crate::skarbiec::{ItemVersion, VersionedValue};

/// One namespace's bearer and the version it was read under.
#[derive(Clone)]
pub(crate) struct CachedObjectToken {
    item: String,
    value: String,
    version: ItemVersion,
}

fn refused(namespace: &str, what: impl std::fmt::Display) {
    eprintln!("[dashboard] object verifier for namespace {namespace}: {what}");
}

impl Dashboard {
    pub(crate) async fn object_token(&self, namespace: &str, item: &str) -> Result<String, ()> {
        let held = self.object_tokens.lock().await.get(namespace).cloned();
        // The version check runs outside the lock, so parallel object
        // requests are not queued behind each other's vault round trip.
        let current = match crate::skarbiec::read_object_token_revision(item, "token").await {
            Ok(current) => current,
            Err(error) => {
                refused(namespace, format!("its version could not be read: {error}"));
                return Err(());
            }
        };
        if let (Some(held), Some(current)) = (&held, &current) {
            if held.item == item && held.version == *current {
                return Ok(held.value.clone());
            }
        }
        // A new version, a first read, or a store without versions: read the
        // bearer. The lock folds a burst that saw the same new version into
        // one vault read: whoever takes it second finds the bearer read.
        let mut tokens = self.object_tokens.lock().await;
        if let (Some(held), Some(current)) = (tokens.get(namespace), &current) {
            if held.item == item && held.version == *current {
                return Ok(held.value.clone());
            }
        }
        match crate::skarbiec::read_object_token_versioned(item, "token").await {
            Ok(Some(VersionedValue { value, version })) if !value.is_empty() => {
                match version {
                    Some(version) => {
                        tokens.insert(
                            namespace.to_string(),
                            CachedObjectToken {
                                item: item.to_string(),
                                value: value.clone(),
                                version,
                            },
                        );
                    }
                    None => {
                        tokens.remove(namespace);
                    }
                }
                Ok(value)
            }
            Ok(_) => {
                tokens.remove(namespace);
                refused(namespace, format_args!("item {item} holds no token"));
                Err(())
            }
            Err(error) => {
                tokens.remove(namespace);
                refused(namespace, format_args!("reading {item} failed: {error}"));
                Err(())
            }
        }
    }

    /// The host-health route's bearer: the `token` of the item that plays
    /// role `host-health-api`, read on every request like the machine,
    /// service and registry verifiers (a host beacon, not object traffic).
    pub(crate) async fn host_health_token(&self) -> Result<String, ()> {
        let role = crate::config::HOST_HEALTH_API_ROLE;
        match crate::skarbiec::read_role_token(role, "token").await {
            Ok(Some(value)) if !value.is_empty() => Ok(value),
            Ok(_) => {
                eprintln!("[dashboard] host-health verifier: no item playing {role} holds a token");
                Err(())
            }
            Err(error) => {
                eprintln!("[dashboard] host-health verifier failed: {error}");
                Err(())
            }
        }
    }

    /// The release publisher item's bearer, read on every request so a
    /// rotation takes effect at once. A read that fails or finds no token is
    /// this service unable to consult its authority (`Err`, a redacted 503):
    /// it used to answer with the last token it had loaded for up to ten
    /// minutes, a stale credential standing in for the vault's answer.
    pub(crate) async fn release_token(&self, item: &str) -> Result<String, ()> {
        match crate::skarbiec::read_release_token(item, "token").await {
            Ok(Some(value)) if !value.is_empty() => Ok(value),
            Ok(_) => {
                eprintln!("[dashboard] release verifier item unavailable: {item}");
                Err(())
            }
            Err(error) => {
                eprintln!("[dashboard] release verifier failed for {item}: {error}");
                Err(())
            }
        }
    }
}
