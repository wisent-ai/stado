//! The durable checkpoint one bounded pass hands to the next: the
//! breadth-first frontier, the first unexamined child of the directory at
//! its front, and the path encoding that survives a non-UTF-8 Unix name.

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Ordinary names stay readable; non-UTF-8 Unix names retain their exact bytes.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub(in crate::providers::local::disk_cleanup) enum CursorPath {
    Text(String),
    Bytes(Vec<u8>),
}

impl CursorPath {
    pub(in crate::providers::local::disk_cleanup) fn as_path(&self) -> &Path {
        match self {
            Self::Text(path) => Path::new(path),
            Self::Bytes(path) => Path::new(OsStr::from_bytes(path)),
        }
    }
}

impl From<PathBuf> for CursorPath {
    fn from(path: PathBuf) -> Self {
        match path.into_os_string().into_string() {
            Ok(path) => Self::Text(path),
            Err(path) => Self::Bytes(path.into_vec()),
        }
    }
}

impl From<CursorPath> for PathBuf {
    fn from(path: CursorPath) -> Self {
        match path {
            CursorPath::Text(path) => Self::from(path),
            CursorPath::Bytes(path) => Self::from(OsString::from_vec(path)),
        }
    }
}

/// Durable breadth-first work queue for a bounded build-cache walk.
///
/// `frontier[0]` is the directory currently being examined; the remaining
/// paths are directories already discovered but not yet opened. `next_child`
/// is the first entry in `frontier[0]` that has not been examined. Persisting
/// both pieces is what makes a deadline a pause rather than a restart: the
/// next pass opens one parent and continues immediately, without rebuilding
/// every shallower level of the tree.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(in crate::providers::local::disk_cleanup) struct BuildCachesCursor {
    pub(super) version: u8,
    pub(super) root: CursorPath,
    pub(super) frontier: VecDeque<CursorPath>,
    pub(super) next_child: Option<CursorPath>,
}

impl BuildCachesCursor {
    pub(super) fn fresh(root: PathBuf) -> Self {
        Self {
            version: 1,
            root: root.into(),
            frontier: VecDeque::from([PathBuf::new().into()]),
            next_child: None,
        }
    }

    pub(super) fn valid_for(&self, root: &Path) -> bool {
        let relative = |path: &Path| {
            path.components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
        };
        self.version == 1
            && self.root.as_path() == root
            && !self.frontier.is_empty()
            && self.frontier.iter().all(|path| relative(path.as_path()))
            && self.next_child.as_ref().is_none_or(|path| {
                let path = path.as_path();
                relative(path)
                    && path.file_name().is_some()
                    && path.parent() == self.frontier.front().map(CursorPath::as_path)
            })
    }

    pub(in crate::providers::local::disk_cleanup) fn from_state(
        state: &serde_json::Value,
    ) -> Option<Self> {
        Self::deserialize(state.get("build_caches_cursor")?).ok()
    }

    pub(in crate::providers::local::disk_cleanup) fn pending_directories(&self) -> usize {
        self.frontier.len()
    }

    pub(in crate::providers::local::disk_cleanup) fn resume_label(&self) -> Option<String> {
        self.next_child
            .as_ref()
            .or_else(|| self.frontier.front())
            .map(|path| path.as_path().to_string_lossy().into_owned())
    }
}
