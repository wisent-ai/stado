//! The namespace bearer cache and the release publisher bearer read. Object
//! traffic must not turn into one Skarbiec read per object request. A vault
//! read that fails is answered as a failure (a redacted 503), never with a
//! token loaded earlier: the last-known-good fallback let a credential the
//! vault could no longer confirm authorize object and release writes for ten
//! minutes.

use std::future::Future;
use std::time::{Duration, Instant};

use crate::dashboard::listener::Dashboard;

const OBJECT_TOKEN_FRESH_FOR: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub(crate) struct CachedObjectToken {
    value: Option<String>,
    loaded_at: Instant,
}

impl Dashboard {
    pub(crate) async fn object_token(&self, namespace: &str, item: &str) -> Result<String, ()> {
        self.verifier_token(namespace, crate::skarbiec::read_object_token(item, "token"))
            .await
    }

    /// The host-health route's bearer: the `token` of the item that plays
    /// role `host-health-api`, cached beside the namespace bearers.
    pub(crate) async fn host_health_token(&self) -> Result<String, ()> {
        self.verifier_token(
            "host-health",
            crate::skarbiec::read_role_token(crate::config::HOST_HEALTH_API_ROLE, "token"),
        )
        .await
    }

    async fn verifier_token(
        &self,
        namespace: &str,
        read: impl Future<Output = Result<Option<String>, crate::skarbiec::SkarbiecError>>,
    ) -> Result<String, ()> {
        let mut tokens = self.object_tokens.lock().await;
        let now = Instant::now();
        if let Some(cached) = tokens.get(namespace) {
            if now.duration_since(cached.loaded_at) <= OBJECT_TOKEN_FRESH_FOR {
                return cached.value.clone().ok_or(());
            }
        }

        match read.await {
            Ok(Some(value)) if !value.is_empty() => {
                tokens.insert(
                    namespace.to_string(),
                    CachedObjectToken {
                        value: Some(value.clone()),
                        loaded_at: now,
                    },
                );
                Ok(value)
            }
            Ok(_) => {
                eprintln!("[dashboard] object verifier item unavailable for namespace {namespace}");
                tokens.insert(
                    namespace.to_string(),
                    CachedObjectToken {
                        value: None,
                        loaded_at: now,
                    },
                );
                Err(())
            }
            Err(error) => {
                eprintln!("[dashboard] object verifier failed for namespace {namespace}: {error}");
                tokens.insert(
                    namespace.to_string(),
                    CachedObjectToken {
                        value: None,
                        loaded_at: now,
                    },
                );
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
