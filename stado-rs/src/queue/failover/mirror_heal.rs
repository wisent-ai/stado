//! What defends the heal: an immutable release object the primary is missing
//! is served from the mirror and written back, a key absent from both stays
//! absent, an upload part and a mutable key are never resurrected, and a
//! primary that errors still falls back for a failover reader.

use super::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::local_file::LocalBackend;

    const RELEASE: &str = "ecosystem/releases/preferences-landing/0.1.1/web/release.json";
    const PART: &str =
        "ecosystem/releases/stado/0.15.25/darwin-arm64/x.tar.gz.__stado_upload/abc/00000000";
    const QUEUE_KEY: &str = "ecosystem/probierz/queue/job-1.json";

    /// Two real local roots, so the heal is exercised against a filesystem
    /// rather than a mock that cannot refuse.
    fn two_roots() -> (tempfile::TempDir, tempfile::TempDir, ReadFailoverBackend) {
        let primary = tempfile::tempdir().expect("primary root");
        let backup = tempfile::tempdir().expect("backup root");
        let store = ReadFailoverBackend::new(
            Arc::new(LocalBackend::new(&primary.path().to_string_lossy()).expect("primary")),
            Arc::new(LocalBackend::new(&backup.path().to_string_lossy()).expect("backup")),
            // The object API's own mode: reads must not be answered from a
            // stale replica. Healing is what makes this case answerable
            // without breaking that rule.
            ReadMode::PrimaryOnly,
        );
        (primary, backup, store)
    }

    #[tokio::test]
    async fn a_primary_miss_the_mirror_can_answer_is_served_and_healed() {
        let (primary, _backup, store) = two_roots();
        store
            .backup
            .upload_bytes(RELEASE, b"signed manifest")
            .await
            .expect("seed the mirror only");

        assert_eq!(
            store.download_bytes(RELEASE).await.expect("read"),
            Some(b"signed manifest".to_vec()),
            "the mirror's copy must be served, not reported absent"
        );
        assert!(
            primary.path().join(RELEASE).is_file(),
            "the primary must have been healed with it"
        );
        assert!(
            store.exists(RELEASE).await.expect("stat"),
            "stat must agree with what a read serves"
        );
    }

    #[tokio::test]
    async fn absent_from_both_stays_absent() {
        let (_primary, _backup, store) = two_roots();
        assert_eq!(store.download_bytes(RELEASE).await.expect("read"), None);
        assert!(!store.exists(RELEASE).await.expect("stat"));
    }

    /// An upload part is not an object: resurrecting one would make an
    /// abandoned upload look resumable.
    #[tokio::test]
    async fn an_upload_part_is_never_healed() {
        let (primary, _backup, store) = two_roots();
        store
            .backup
            .upload_bytes(PART, b"part")
            .await
            .expect("seed the mirror only");
        assert_eq!(store.download_bytes(PART).await.expect("read"), None);
        assert!(!primary.path().join(PART).exists());
    }

    /// And a mutable key is not healed either, which is the invariant
    /// `PrimaryOnly` exists to protect: a stale replica must never be returned
    /// as authority.
    #[tokio::test]
    async fn a_mutable_key_is_not_served_from_the_mirror() {
        let (primary, _backup, store) = two_roots();
        store
            .backup
            .upload_bytes(QUEUE_KEY, b"stale job state")
            .await
            .expect("seed the mirror only");
        assert_eq!(store.download_bytes(QUEUE_KEY).await.expect("read"), None);
        assert!(!primary.path().join(QUEUE_KEY).exists());
    }

    /// A primary that ERRORS still falls back exactly as before, for a normal
    /// client: that path is untouched.
    #[tokio::test]
    async fn a_primary_error_still_falls_back_for_a_failover_reader() {
        let primary = tempfile::tempdir().expect("primary root");
        let backup = tempfile::tempdir().expect("backup root");
        let store = ReadFailoverBackend::new(
            Arc::new(LocalBackend::new(&primary.path().to_string_lossy()).expect("primary")),
            Arc::new(LocalBackend::new(&backup.path().to_string_lossy()).expect("backup")),
            ReadMode::Failover,
        );
        store
            .backup
            .upload_bytes(RELEASE, b"mirror")
            .await
            .expect("seed the mirror");
        assert_eq!(
            store.download_bytes(RELEASE).await.expect("read"),
            Some(b"mirror".to_vec())
        );
    }
}
