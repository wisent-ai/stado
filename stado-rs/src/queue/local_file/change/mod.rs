//! The kernel's own notification that a local store directory changed: kqueue
//! on macOS, inotify on Linux. A held read arms the watch before it reads, so
//! a write that lands between the read and the wait still wakes it, and it
//! wakes for writes by any process on this device, not only this one.

use std::path::PathBuf;

use crate::queue::{ChangeWatch, StorageError};

use super::LocalBackend;

impl LocalBackend {
    /// The directory of each prefix, created when absent so a prefix nothing
    /// has written to yet can still be watched.
    pub(super) fn watched_directories(
        &self,
        prefixes: &[&str],
    ) -> Result<Vec<PathBuf>, StorageError> {
        prefixes
            .iter()
            .map(|prefix| {
                let directory = self.path(prefix.trim_end_matches('/'))?;
                std::fs::create_dir_all(&directory).map_err(|error| {
                    StorageError::Other(format!(
                        "cannot create {} to watch it: {error}",
                        directory.display()
                    ))
                })?;
                Ok(directory)
            })
            .collect()
    }

    pub(super) fn arm_watch(
        &self,
        prefixes: &[&str],
    ) -> Result<Box<dyn ChangeWatch>, StorageError> {
        let directories = self.watched_directories(prefixes)?;
        platform::arm(&directories).map(|watch| Box::new(watch) as Box<dyn ChangeWatch>)
    }
}

/// The kernel's notification for writes under `directories`, for a reader
/// that waits on a file another Stado process publishes — the resolver's
/// state, say — rather than on a store prefix. Armed before the caller's
/// read, as the store's own watches are, so a write that lands between the
/// read and the wait still wakes it.
pub(crate) fn watch(directories: &[PathBuf]) -> Result<Box<dyn ChangeWatch>, StorageError> {
    platform::arm(directories).map(|watch| Box::new(watch) as Box<dyn ChangeWatch>)
}

fn kernel_error(action: &str, directory: &std::path::Path, error: nix::Error) -> StorageError {
    StorageError::Other(format!(
        "{action} the change watch on {}: {error}",
        directory.display()
    ))
}

#[cfg(target_os = "macos")]
mod platform {
    use std::fs::File;
    use std::os::fd::AsRawFd;
    use std::path::{Path, PathBuf};

    use nix::sys::event::{EvFlags, EventFilter, FilterFlag, KEvent, Kqueue};

    use super::kernel_error;
    use crate::queue::{ChangeWatch, StorageError};

    pub(super) struct Watch {
        queue: Kqueue,
        /// Held open: kqueue watches a vnode through this descriptor.
        _directories: Vec<File>,
        first: PathBuf,
    }

    pub(super) fn arm(directories: &[PathBuf]) -> Result<Watch, StorageError> {
        let first = directories.first().cloned().unwrap_or_default();
        let queue = Kqueue::new().map_err(|error| kernel_error("open", &first, error))?;
        let mut opened = Vec::with_capacity(directories.len());
        let mut changes = Vec::with_capacity(directories.len());
        for directory in directories {
            let file = File::open(directory).map_err(|error| {
                StorageError::Other(format!(
                    "cannot open {} to watch it: {error}",
                    directory.display()
                ))
            })?;
            changes.push(KEvent::new(
                file.as_raw_fd() as usize,
                EventFilter::EVFILT_VNODE,
                EvFlags::EV_ADD | EvFlags::EV_CLEAR,
                FilterFlag::NOTE_WRITE | FilterFlag::NOTE_DELETE | FilterFlag::NOTE_RENAME,
                0,
                0,
            ));
            opened.push(file);
        }
        queue
            .kevent(&changes, &mut [], None)
            .map_err(|error| kernel_error("register", &first, error))?;
        Ok(Watch {
            queue,
            _directories: opened,
            first,
        })
    }

    impl ChangeWatch for Watch {
        fn next(&mut self) -> Result<(), StorageError> {
            let mut events = [KEvent::new(
                0,
                EventFilter::EVFILT_VNODE,
                EvFlags::empty(),
                FilterFlag::empty(),
                0,
                0,
            )];
            let first: &Path = &self.first;
            self.queue
                .kevent(&[], &mut events, None)
                .map(|_| ())
                .map_err(|error| kernel_error("wait on", first, error))
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::path::PathBuf;

    use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};

    use super::kernel_error;
    use crate::queue::{ChangeWatch, StorageError};

    pub(super) struct Watch {
        inotify: Inotify,
        first: PathBuf,
    }

    pub(super) fn arm(directories: &[PathBuf]) -> Result<Watch, StorageError> {
        let first = directories.first().cloned().unwrap_or_default();
        let inotify = Inotify::init(InitFlags::IN_CLOEXEC)
            .map_err(|error| kernel_error("open", &first, error))?;
        for directory in directories {
            inotify
                .add_watch(
                    directory.as_path(),
                    AddWatchFlags::IN_CREATE
                        | AddWatchFlags::IN_MOVED_TO
                        | AddWatchFlags::IN_CLOSE_WRITE
                        | AddWatchFlags::IN_DELETE,
                )
                .map_err(|error| kernel_error("register", directory, error))?;
        }
        Ok(Watch { inotify, first })
    }

    impl ChangeWatch for Watch {
        fn next(&mut self) -> Result<(), StorageError> {
            self.inotify
                .read_events()
                .map(|_| ())
                .map_err(|error| kernel_error("wait on", &self.first, error))
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod platform {
    use std::path::PathBuf;

    use crate::queue::StorageError;

    pub(super) struct Watch;

    impl crate::queue::ChangeWatch for Watch {
        fn next(&mut self) -> Result<(), StorageError> {
            Ok(())
        }
    }

    pub(super) fn arm(_directories: &[PathBuf]) -> Result<Watch, StorageError> {
        Err(StorageError::Other(format!(
            "the local storage backend has no change watch on {}",
            std::env::consts::OS
        )))
    }
}
