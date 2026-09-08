//! The namespace and release publisher bearer caches. Object traffic must not
//! turn into one Skarbiec read per object request.

use std::time::{Duration, Instant};

use crate::dashboard::listener::Dashboard;

const OBJECT_TOKEN_FRESH_FOR: Duration = Duration::from_secs(60);
const OBJECT_TOKEN_STALE_FOR: Duration = Duration::from_secs(10 * 60);

#[derive(Clone)]
pub(crate) struct CachedObjectToken {
    value: Option<String>,
    loaded_at: Instant,
}

impl Dashboard {
    pub(crate) async fn object_token(&self, namespace: &str, item: &str) -> Result<String, ()> {
        let mut tokens = self.object_tokens.lock().await;
        let now = Instant::now();
        if let Some(cached) = tokens.get(namespace) {
            if now.duration_since(cached.loaded_at) <= OBJECT_TOKEN_FRESH_FOR {
                return cached.value.clone().ok_or(());
            }
        }

        match crate::skarbiec::read_object_token(item, "token").await {
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
                if let Some(cached) = tokens.get(namespace) {
                    if let Some(value) = &cached.value {
                        if now.duration_since(cached.loaded_at) <= OBJECT_TOKEN_STALE_FOR {
                            eprintln!(
                                "[dashboard] object verifier refresh failed for namespace {namespace}; using the last token loaded {}s ago: {error}",
                                now.duration_since(cached.loaded_at).as_secs()
                            );
                            return Ok(value.clone());
                        }
                    }
                }
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
    pub(crate) async fn release_token(&self, item: &str) -> Result<String, ()> {
        let mut tokens = self.release_tokens.lock().await;
        let now = Instant::now();
        if let Some(cached) = tokens.get(item) {
            if now.duration_since(cached.loaded_at) <= OBJECT_TOKEN_FRESH_FOR {
                return cached.value.clone().ok_or(());
            }
        }

        match crate::skarbiec::read_release_token(item, "token").await {
            Ok(Some(value)) if !value.is_empty() => {
                tokens.insert(
                    item.to_string(),
                    CachedObjectToken {
                        value: Some(value.clone()),
                        loaded_at: now,
                    },
                );
                Ok(value)
            }
            Ok(_) => {
                if let Some(cached) = tokens.get(item) {
                    if let Some(value) = &cached.value {
                        if now.duration_since(cached.loaded_at) <= OBJECT_TOKEN_STALE_FOR {
                            eprintln!(
                                "[dashboard] release verifier item unavailable for {item}; using \
                                 the last token loaded {}s ago",
                                now.duration_since(cached.loaded_at).as_secs()
                            );
                            return Ok(value.clone());
                        }
                    }
                }
                eprintln!("[dashboard] release verifier item unavailable: {item}");
                tokens.insert(
                    item.to_string(),
                    CachedObjectToken {
                        value: None,
                        loaded_at: now,
                    },
                );
                Err(())
            }
            Err(error) => {
                if let Some(cached) = tokens.get(item) {
                    if let Some(value) = &cached.value {
                        if now.duration_since(cached.loaded_at) <= OBJECT_TOKEN_STALE_FOR {
                            eprintln!(
                                "[dashboard] release verifier refresh failed for {item}; using the \
                                 last token loaded {}s ago: {error}",
                                now.duration_since(cached.loaded_at).as_secs()
                            );
                            return Ok(value.clone());
                        }
                    }
                }
                eprintln!("[dashboard] release verifier failed for {item}: {error}");
                tokens.insert(
                    item.to_string(),
                    CachedObjectToken {
                        value: None,
                        loaded_at: now,
                    },
                );
                Err(())
            }
        }
    }
}
