use crate::targets::*;

/// Store-relative `registry.json` download: `Ok(Some(text))` = fetched with
/// the store's generation token, `Ok(None)` = blob absent (Python
/// `blob.generation is None`), `Err(msg)` = the store could not be reached
/// at all.
///
/// The generation travels with the text because the last-known-good cache
/// records WHICH document it is holding ([`RegistryCacheMeta::generation`]):
/// a cache that can only say "some registry, once" cannot be compared with
/// the authority when the authority comes back.
pub type RegistryDownloader =
    Arc<dyn Fn() -> BoxFuture<'static, Result<Option<VersionedText>, String>> + Send + Sync>;

/// Test seam replacing the production download (loopback mocks).
static REGISTRY_DOWNLOADER: LazyLock<Mutex<Option<RegistryDownloader>>> =
    LazyLock::new(|| Mutex::new(None));

/// Install a downloader in place of the production fetch (tests only —
/// `#[doc(hidden)]`, not part of the crate's operational surface). Serialize
/// via `testutil::GLOBAL_STATE_LOCK`.
#[doc(hidden)]
pub fn set_registry_downloader_for_testing(downloader: Option<RegistryDownloader>) {
    // Replaced whole, as the reader below takes it back through poison.
    let mut slot = match REGISTRY_DOWNLOADER.lock() {
        Ok(slot) => slot,
        Err(poisoned) => poisoned.into_inner(),
    };
    *slot = downloader;
}

pub(crate) async fn download_registry() -> Result<Option<VersionedText>, String> {
    // One whole value at every moment, so a poisoned lock is taken over.
    let downloader = match REGISTRY_DOWNLOADER.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    match downloader {
        Some(downloader) => downloader().await,
        None => super::download_registry_blob().await,
    }
}
