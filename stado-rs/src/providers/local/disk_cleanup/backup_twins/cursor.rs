//! Resume a bounded replica scan after retained files instead of visiting the
//! same prefix forever. The checkpoint is a location hint, never deletion proof.

use std::collections::VecDeque;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use super::super::build_caches::cursor::CursorPath;
use super::super::{euid, CleanupReport, JanitorError};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(in crate::providers::local::disk_cleanup) struct BackupCursor {
    root: CursorPath,
    frontier: VecDeque<CursorPath>,
    after: Option<CursorPath>,
}

impl BackupCursor {
    pub(in crate::providers::local::disk_cleanup) fn from_state(
        state: &serde_json::Value,
    ) -> Option<Self> {
        Self::deserialize(state.get("backup_twins_cursor")?).ok()
    }

    fn valid_for(&self, root: &Path) -> bool {
        let relative = |path: &Path| {
            path.components()
                .all(|part| matches!(part, Component::Normal(_)))
        };
        self.root.as_path() == root
            && !self.frontier.is_empty()
            && self.frontier.iter().all(|path| relative(path.as_path()))
            && self.after.as_ref().is_none_or(|path| {
                relative(path.as_path())
                    && path.as_path().parent() == self.frontier.front().map(CursorPath::as_path)
            })
    }
}

pub(super) struct Walk {
    cursor: BackupCursor,
    children: Option<std::vec::IntoIter<PathBuf>>,
    remaining: i64,
    deadline: Instant,
    device: u64,
}

impl Walk {
    pub(super) fn new(
        root: &Path,
        previous: Option<BackupCursor>,
        remaining: i64,
        deadline: Instant,
        device: u64,
    ) -> Self {
        let cursor = previous
            .filter(|cursor| cursor.valid_for(root))
            .unwrap_or_else(|| BackupCursor {
                root: root.to_path_buf().into(),
                frontier: VecDeque::from([PathBuf::new().into()]),
                after: None,
            });
        Self {
            cursor,
            children: None,
            remaining,
            deadline,
            device,
        }
    }

    pub(super) fn next(&mut self, report: &mut CleanupReport) -> Option<PathBuf> {
        while let Some(parent) = self
            .cursor
            .frontier
            .front()
            .map(|path| path.as_path().to_path_buf())
        {
            if self.remaining <= 0 || Instant::now() >= self.deadline {
                if self.remaining <= 0 {
                    report.caps.scan = true;
                    report.skip_backup_twins("scan_cap", 1);
                } else {
                    report.caps.deadline = true;
                    report.skip_backup_twins("scan_deadline", 1);
                }
                return None;
            }
            let root = self.cursor.root.as_path();
            if self.children.is_none() {
                let entries = self.read_children(root, &parent);
                match entries {
                    Ok(children) => self.children = Some(children.into_iter()),
                    Err(error) => {
                        report.add_error(super::CLEANER, &error);
                        report.skip_backup_twins("read_dir_failed", 1);
                        self.cursor.frontier.pop_front();
                        self.cursor.after = None;
                        continue;
                    }
                }
            }
            let child = self.children.as_mut().and_then(Iterator::next);
            let Some(relative) = child else {
                self.children = None;
                self.cursor.frontier.pop_front();
                self.cursor.after = None;
                continue;
            };
            self.remaining -= 1;
            report.backup_twins.scanned_items += 1;
            self.cursor.after = Some(relative.clone().into());
            let path = root.join(&relative);
            let info = match std::fs::symlink_metadata(&path) {
                Ok(info) => info,
                Err(error) => {
                    report.add_error(super::CLEANER, &error.into());
                    continue;
                }
            };
            if info.file_type().is_symlink() || info.uid() != euid() || info.dev() != self.device {
                report.skip_backup_twins("not_a_plain_owned_file", 1);
            } else if info.is_dir() {
                self.cursor.frontier.push_back(relative.into());
            } else if info.is_file() {
                return Some(path);
            }
        }
        None
    }

    fn read_children(&self, root: &Path, parent: &Path) -> Result<Vec<PathBuf>, JanitorError> {
        let mut path = root.to_path_buf();
        for component in std::iter::once(None).chain(parent.components().map(Some)) {
            if let Some(component) = component {
                path.push(component);
            }
            let info = std::fs::symlink_metadata(&path)?;
            if !info.is_dir()
                || info.file_type().is_symlink()
                || info.uid() != euid()
                || info.dev() != self.device
            {
                return Err(JanitorError::os(
                    "replica scan directory is no longer owned on this device",
                ));
            }
        }
        let mut children = Vec::new();
        for entry in std::fs::read_dir(path)? {
            let relative = parent.join(entry?.file_name());
            if self
                .cursor
                .after
                .as_ref()
                .is_none_or(|after| relative.as_path() > after.as_path())
            {
                children.push(relative);
            }
        }
        children.sort();
        Ok(children)
    }

    pub(super) fn checkpoint(self) -> Option<BackupCursor> {
        (!self.cursor.frontier.is_empty()).then_some(self.cursor)
    }
}
