//! The replica walk: every regular file this account owns under the replica
//! root, on the home volume, in a stable breadth-first order.

use std::collections::VecDeque;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::super::{euid, CleanupReport, JanitorError};

pub(super) struct Walk {
    root: PathBuf,
    frontier: VecDeque<PathBuf>,
    children: Option<std::vec::IntoIter<PathBuf>>,
    device: u64,
}

impl Walk {
    pub(super) fn new(root: &Path, device: u64) -> Self {
        Self {
            root: root.to_path_buf(),
            frontier: VecDeque::from([PathBuf::new()]),
            children: None,
            device,
        }
    }

    pub(super) fn next(&mut self, report: &mut CleanupReport) -> Option<PathBuf> {
        while let Some(parent) = self.frontier.front().cloned() {
            if self.children.is_none() {
                match self.read_children(&parent) {
                    Ok(children) => self.children = Some(children.into_iter()),
                    Err(error) => {
                        report.add_error(super::CLEANER, &error);
                        report.skip_backup_twins("read_dir_failed", 1);
                        self.frontier.pop_front();
                        continue;
                    }
                }
            }
            let Some(relative) = self.children.as_mut().and_then(Iterator::next) else {
                self.children = None;
                self.frontier.pop_front();
                continue;
            };
            report.backup_twins.scanned_items += 1;
            let path = self.root.join(&relative);
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
                self.frontier.push_back(relative);
            } else if info.is_file() {
                return Some(path);
            }
        }
        None
    }

    fn read_children(&self, parent: &Path) -> Result<Vec<PathBuf>, JanitorError> {
        let mut path = self.root.clone();
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
            children.push(parent.join(entry?.file_name()));
        }
        children.sort();
        Ok(children)
    }
}
