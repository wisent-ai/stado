use crate::targets::*;

/// Whether this read is answering from a local filesystem store.
///
/// The last-known-good copy exists for one situation: the authority is on the
/// other side of a network that is not answering. A store that is a directory
/// on this disk has no such state — if the path is gone, so is the copy beside
/// it — and treating one as the fleet's authority costs the operator the
/// fallback the fleet does depend on.
///
/// That is not hypothetical. `stado scratch` emits a one-target registry into a
/// storage root of its own, so every command a test drives through that lease
/// used to record it as the last-known-good registry of this machine: fifty
/// seven declared hosts replaced by one, in the copy every host command falls
/// back to when the store goes quiet. `may_replace_last_good` refuses only a
/// document naming zero hosts, so one host sailed through. Reading is scoped
/// the same way and for the sharper reason: a lease whose own document was
/// missing would otherwise be served the operator's fleet and drive real
/// commands at it.
fn store_is_local_filesystem() -> bool {
    crate::capabilities::storage_adapter(crate::config::wc_storage_backend())
        == Some(crate::capabilities::StorageAdapter::Local)
}

/// What a degraded registry read is answering from: the authority's own
/// refusal and the identity of the cached copy being served. Carried
/// structured so a caller can word its own one-line notice
/// (`fetch_registry_or_last_good` keeps the historical sentence in
/// [`RegistryCopyNotice::notice`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryCopyNotice {
    /// The historical one-sentence notice (age, copy path, read_at,
    /// generation, authority error).
    pub notice: String,
    /// The authority's own error text.
    pub cause: String,
    /// When the authority served the copy being read (RFC3339).
    pub read_at: String,
    /// Age of the copy in seconds.
    pub age_seconds: i64,
}

/// The registry for a reader that must keep answering when the authority
/// does not: the canonical document first, then the last-known-good copy.
///
/// The second element is the one sentence the caller MUST put in front of
/// the operator when the answer came from the copy. It names the age of what
/// is being read and keeps the authority's own error text, because "this
/// registry is 412 s old" and "the store is unreachable" send an operator to
/// two different places — and the behavior this replaces sent them to
/// neither: every host command died with one line about the store while the
/// question was which host went silent.
///
/// A `Some` sentence and `Registry::staleness_seconds` move together. `Err`
/// means the authority failed and there is no usable copy; the bundled
/// snapshot is below this, in [`load_registry_auto`].
pub async fn fetch_registry_or_last_good() -> Result<(Registry, Option<String>), RegistryFetchError>
{
    let (registry, copy) = fetch_registry_or_last_good_detail().await?;
    Ok((registry, copy.map(|copy| copy.notice)))
}

/// [`fetch_registry_or_last_good`] with the fallback carried structured:
/// the authority's refusal and the copy's identity as separate fields, for
/// callers whose operator contract names its own sentence.
pub async fn fetch_registry_or_last_good_detail(
) -> Result<(Registry, Option<RegistryCopyNotice>), RegistryFetchError> {
    let authority = match fetch_registry_remote().await {
        Ok(registry) => return Ok((registry, None)),
        Err(error) => error,
    };
    match load_last_good().filter(|_| !store_is_local_filesystem()) {
        Some((registry, meta, age)) => {
            let mut notice = format!(
                "reading the last-known-good registry copy from {age}s ago ({}, read_at {}, generation {}) because the authority did not answer: {authority}",
                registry_last_good_path().unwrap_or_default().display(),
                meta.read_at,
                meta.generation,
            );
            // Why the copy is as old as it is, when this process already
            // knows. Without it the age looks like the authority's fault,
            // and an operator chasing an unreachable store never learns
            // that the last refresh reached this host and was refused.
            if let Some(refusal) = last_good_refusal() {
                notice.push_str(&format!(
                    "; this process refused to refresh that copy ({}): {refusal}",
                    refusal.kind()
                ));
            }
            Ok((
                registry,
                Some(RegistryCopyNotice {
                    notice,
                    cause: authority.to_string(),
                    read_at: meta.read_at,
                    age_seconds: age,
                }),
            ))
        }
        None => Err(authority),
    }
}

/// Fetch the canonical registry from the configured store (Python
/// `_load_from_gcs`, `source="gcs"`): the authority for fleet-survival
/// decisions — the coordinator's rogue-daemon kill switch and host-health
/// target resolution. Cached for [`GCS_REGISTRY_TTL_SEC`] seconds; only
/// successful fetches are cached, so a failure is retried on the next call
/// (Python parity).
///
/// Returns [`RegistryFetchError`] rather than an empty registry: a caller
/// MUST NOT read "the store is unreachable" as "the entry is gone". There
/// is still no local escape hatch here — the bundled file is reachable only
/// through [`load_registry_auto`].
pub async fn fetch_registry_remote() -> Result<Registry, RegistryFetchError> {
    if let Some((ts, registry)) = &*REGISTRY_CACHE.lock().expect("registry cache lock") {
        if ts.elapsed() < Duration::from_secs(GCS_REGISTRY_TTL_SEC) {
            return Ok(registry.clone());
        }
    }
    let fetched = fetch_registry_remote_uncached().await;
    if let Ok(registry) = &fetched {
        *REGISTRY_CACHE.lock().expect("registry cache lock") =
            Some((Instant::now(), registry.clone()));
    }
    fetched
}

async fn fetch_registry_remote_uncached() -> Result<Registry, RegistryFetchError> {
    let location = registry_location();
    match download_registry().await {
        Ok(Some(document)) => match load_registry_from_str(&document.content) {
            Ok(registry) => {
                // The authority answered, so this read is not degraded and
                // the refusal must not fail it. What it must not do is
                // vanish: the copy on disk is now older than the document
                // just served, and the sentence this process prints when it
                // later falls back to that copy is the one place an operator
                // sees the consequence.
                if !store_is_local_filesystem() {
                    if let Err(refusal) = store_last_good(&document.content, &document.version) {
                        note_last_good_refusal(&refusal);
                    }
                }
                Ok(registry)
            }
            // The `[_load_from_gcs]` prefix is Python's function name, kept
            // verbatim so existing operator log greps still match.
            Err(source) => {
                eprintln!("[_load_from_gcs] failed: {source}");
                Err(RegistryFetchError::Invalid { location, source })
            }
        },
        // Ok(None) = blob absent (Python `blob.generation is None`).
        Ok(None) => Err(RegistryFetchError::Absent { location }),
        Err(detail) => {
            eprintln!("[_load_from_gcs] failed: {detail}");
            Err(RegistryFetchError::Unreachable { location, detail })
        }
    }
}

/// The registry document, authority first, then the last-known-good copy,
/// then the file bundled with this binary (Python `load_targets` /
/// `load_coordinators` with `source="auto"`). Every [`RegistryFetchError`]
/// falls through.
///
/// Announces which of the three it read whenever that is not the authority:
/// the bundled snapshot is a build artifact that can be months old, and
/// answering out of it in silence is how a decommissioned host stayed
/// "declared" for a fortnight.
pub async fn load_registry_auto() -> Result<Registry, RegistryError> {
    match fetch_registry_or_last_good().await {
        Ok((registry, notice)) => {
            if let Some(notice) = notice {
                report_registry_notice(&notice);
            }
            Ok(registry)
        }
        Err(authority) => {
            report_registry_notice(&format!(
                "reading the registry snapshot bundled with this binary because the authority did not answer and there is no last-known-good copy at {}: {authority}",
                registry_last_good_path().unwrap_or_default().display(),
            ));
            load_bundled_registry()
        }
    }
}
