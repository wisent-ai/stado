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
//! A vault read that fails is answered as a failure (a 503 carrying the
//! server's own wait line, `super::vault`), never with a bearer held from
//! earlier. A store that keeps no versions (the file backend) has its bearer
//! read on every request. The bearer read holds its namespace while it runs:
//! requests used to queue behind it on this cache's lock, and with the vault
//! waiting on a held key database every one of them then paid that wait in
//! turn; a request that finds the namespace held is refused at once instead,
//! naming the read it would have stood behind.

use crate::dashboard::listener::Dashboard;
use crate::skarbiec::{ItemVersion, SkarbiecError, VersionedValue};

use super::AuthorityUnavailable;

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
    pub(crate) async fn object_token(
        &self,
        namespace: &str,
        item: &str,
    ) -> Result<String, AuthorityUnavailable> {
        let held = self.object_tokens.lock().await.get(namespace).cloned();
        // The version check runs beside every other request's: a shared
        // consultation, refused only while the namespace's bearer is being
        // read or after a consultation of it failed.
        let current = self
            .consult_vault(
                namespace,
                false,
                format!("the version of the bearer of namespace {namespace} (item {item})"),
                crate::skarbiec::read_object_token_revision(item, "token"),
            )
            .await
            .inspect_err(|unavailable| {
                refused(
                    namespace,
                    format_args!("its version could not be read: {}", unavailable.cause),
                );
            })?;
        if let (Some(held), Some(current)) = (&held, &current) {
            if held.item == item && held.version == *current {
                return Ok(held.value.clone());
            }
        }
        // A new version, a first read, or a store without versions: read the
        // bearer, holding the namespace so a burst that saw the same new
        // version makes one vault read; whoever comes second is refused with
        // this read's own wait line rather than queued behind it.
        let read = self
            .consult_vault(
                namespace,
                true,
                format!("the bearer of namespace {namespace} (item {item})"),
                self.read_object_bearer(namespace, item, current.as_ref()),
            )
            .await;
        let mut tokens = self.object_tokens.lock().await;
        match read {
            Ok(Some(VersionedValue { value, version })) => {
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
            Ok(None) => {
                tokens.remove(namespace);
                refused(namespace, format_args!("item {item} holds no token"));
                Err(AuthorityUnavailable::new(format!(
                    "item {item} holds no token"
                )))
            }
            Err(unavailable) => {
                tokens.remove(namespace);
                refused(
                    namespace,
                    format_args!("reading {item} failed: {}", unavailable.cause),
                );
                Err(unavailable)
            }
        }
    }

    /// The bearer itself, unless the version a peer read between this
    /// request's version check and its claim of the namespace already sits
    /// in the cache: then that bearer, with no vault read.
    async fn read_object_bearer(
        &self,
        namespace: &str,
        item: &str,
        current: Option<&ItemVersion>,
    ) -> Result<Option<VersionedValue>, SkarbiecError> {
        if let (Some(held), Some(current)) =
            (self.object_tokens.lock().await.get(namespace), current)
        {
            if held.item == item && held.version == *current {
                return Ok(Some(VersionedValue {
                    value: held.value.clone(),
                    version: Some(held.version.clone()),
                }));
            }
        }
        match crate::skarbiec::read_object_token_versioned(item, "token").await? {
            Some(versioned) if !versioned.value.is_empty() => Ok(Some(versioned)),
            _ => Ok(None),
        }
    }

    /// The host-health route's bearer: the `token` of the item that plays
    /// role `host-health-api`, read on every request like the machine,
    /// service and registry verifiers (a host beacon, not object traffic).
    pub(crate) async fn host_health_token(&self) -> Result<String, AuthorityUnavailable> {
        let role = crate::config::HOST_HEALTH_API_ROLE;
        let read = self
            .consult_vault(
                "host-health",
                false,
                format!("the token of the item playing role {role}"),
                crate::skarbiec::read_role_token(role, "token"),
            )
            .await;
        match read {
            Ok(Some(value)) if !value.is_empty() => Ok(value),
            Ok(_) => {
                let cause = format!("no item playing {role} holds a token");
                eprintln!("[dashboard] host-health verifier: {cause}");
                Err(AuthorityUnavailable::new(cause))
            }
            Err(unavailable) => {
                eprintln!(
                    "[dashboard] host-health verifier failed: {}",
                    unavailable.cause
                );
                Err(unavailable)
            }
        }
    }

    /// The release publisher item's bearer, read on every request so a
    /// rotation takes effect at once. A read that fails or finds no token is
    /// this service unable to consult its authority (a 503 naming the cause):
    /// it used to answer with the last token it had loaded for up to ten
    /// minutes, a stale credential standing in for the vault's answer.
    pub(crate) async fn release_token(&self, item: &str) -> Result<String, AuthorityUnavailable> {
        let read = self
            .consult_vault(
                &format!("release:{item}"),
                false,
                format!("the token of release publisher item {item}"),
                crate::skarbiec::read_release_token(item, "token"),
            )
            .await;
        match read {
            Ok(Some(value)) if !value.is_empty() => Ok(value),
            Ok(_) => {
                let cause = format!("release verifier item {item} holds no token");
                eprintln!("[dashboard] {cause}");
                Err(AuthorityUnavailable::new(cause))
            }
            Err(unavailable) => {
                eprintln!(
                    "[dashboard] release verifier failed for {item}: {}",
                    unavailable.cause
                );
                Err(unavailable)
            }
        }
    }
}
