//! Where a process roots a local store: at its top, or in the served queue
//! namespace under it.

use crate::config;

/// Where a process roots a local store.
///
/// A store an object API serves keeps the fleet's queue under
/// `ecosystem/<QUEUE_OBJECT_NAMESPACE>/`, the keys every remote agent reads
/// and writes through that API. The server itself addresses the whole store
/// (`Served`); every other process on that host — the queue agent inside the
/// object API among them — is a queue client (`Client`) and roots its queue
/// in that namespace. Rooted at the store top, the serving host's agent
/// published its capacity where no other host looks and claimed none of the
/// fleet's jobs. A store no object API serves has no namespace directory and
/// stays rooted at its top.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StoreRoot {
    Served,
    Client,
}

impl StoreRoot {
    pub(super) fn namespace_tail() -> std::path::PathBuf {
        std::path::Path::new("ecosystem").join(config::QUEUE_OBJECT_NAMESPACE)
    }

    pub(super) fn namespaced(root: &std::path::Path) -> std::path::PathBuf {
        root.join(Self::namespace_tail())
    }

    /// The directory this process's queue lives in, under a resolved root.
    pub(crate) fn queue_root(self, root: &std::path::Path) -> std::path::PathBuf {
        let served = Self::namespaced(root);
        if self == Self::Client && served.is_dir() {
            served
        } else {
            root.to_path_buf()
        }
    }

    /// The top of the store a queue client on the serving host is rooted
    /// under, when `queue_root` put it in the served queue namespace. Objects
    /// of other namespaces — a published release among them — live under
    /// that top, not under the client's queue directory.
    pub(crate) fn served_top(client_root: &std::path::Path) -> Option<std::path::PathBuf> {
        let tail = Self::namespace_tail();
        client_root
            .ends_with(&tail)
            .then(|| {
                client_root
                    .ancestors()
                    .nth(tail.components().count())
                    .map(std::path::Path::to_path_buf)
            })
            .flatten()
    }
}
