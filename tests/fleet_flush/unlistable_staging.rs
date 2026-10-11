//! A fleet staging directory that exists but cannot be listed is not an
//! empty one: the flush refuses with `cannot list the staging directory
//! <dir>: <cause>` carrying the operating system's own error kind, starts no
//! flush process and rotates nothing, instead of answering "nothing to
//! flush" over staged data it never saw. An empty staging that can be
//! listed still answers that there is nothing to flush.
//!
//! The product's explicit-interpreter seam is driven against a staging
//! directory under the checkout's build directory whose permissions this
//! test takes away and restores, so the flush runner (which is never
//! reached) needs no interpreter.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use nix::sys::stat::Mode;
use stado::providers::local::disk::fleet_flush::spawn_fleet_flush_with;

/// The permission bits of a mode, apart from its file type.
fn permission_bits() -> u32 {
    u32::from((Mode::S_IRWXU | Mode::S_IRWXG | Mode::S_IRWXO).bits())
}

const CANNOT_LIST: &str = "cannot list the staging directory";

struct Staging {
    root: PathBuf,
    dir: PathBuf,
    readable: u32,
}

impl Staging {
    fn new(name: &str) -> Self {
        let parent = Path::new(env!("CARGO_TARGET_TMPDIR")).join("fleet-flush-unlistable");
        fs::create_dir_all(&parent).expect("create the test output directory");
        let root = parent.join(format!("{}-{name}", std::process::id()));
        let dir = root.join("fleet_staging");
        fs::create_dir_all(&dir).expect("create the staging directory");
        let readable = fs::metadata(&dir).unwrap().permissions().mode() & permission_bits();
        Self {
            root,
            dir,
            readable,
        }
    }

    /// Take every permission from the staging directory, so listing it is
    /// refused by the operating system.
    fn make_unlistable(&self) {
        let mut permissions = fs::metadata(&self.dir).unwrap().permissions();
        permissions.set_mode(permissions.mode() & !permission_bits());
        fs::set_permissions(&self.dir, permissions).expect("take the staging permissions");
    }

    fn flush(&self) -> std::io::Result<bool> {
        let mut log = Vec::new();
        let answer = spawn_fleet_flush_with(env!("CARGO_BIN_EXE_stado"), &self.dir, &mut |line| {
            log.push(line.to_string())
        });
        assert!(
            log.is_empty(),
            "no flush was started before the staging directory was judged: {log:?}"
        );
        answer
    }

    fn flush_root(&self) -> PathBuf {
        self.root.join("fleet_staging.async_flushes")
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let mut permissions = match fs::metadata(&self.dir) {
            Ok(metadata) => metadata.permissions(),
            Err(_) => return,
        };
        permissions.set_mode((permissions.mode() & !permission_bits()) | self.readable);
        let _ = fs::set_permissions(&self.dir, permissions);
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn a_staging_directory_that_cannot_be_listed_is_refused_with_its_cause_not_read_as_empty() {
    let staging = Staging::new("unlistable");
    fs::write(staging.dir.join("staged.bin"), b"staged job data").unwrap();
    staging.make_unlistable();

    let error = match staging.flush() {
        Err(error) => error,
        Ok(started) => panic!(
            "an unlistable staging directory was read as {} instead of refused",
            if started { "a running flush" } else { "empty" }
        ),
    };
    assert_eq!(
        error.kind(),
        std::io::ErrorKind::PermissionDenied,
        "the operating system's own kind is kept: {error}"
    );
    let said = error.to_string();
    assert!(
        said.contains(CANNOT_LIST) && said.contains(&staging.dir.display().to_string()),
        "the refusal names the staging directory it could not list: {said}"
    );
    let rotated: Vec<PathBuf> = fs::read_dir(staging.flush_root())
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    assert!(
        rotated.is_empty(),
        "nothing was rotated into a flush while staging could not be listed: {rotated:?}"
    );
}

#[test]
fn an_empty_staging_directory_that_can_be_listed_has_nothing_to_flush() {
    let staging = Staging::new("empty");
    match staging.flush() {
        Ok(started) => assert!(!started, "an empty staging directory started a flush"),
        Err(error) => panic!("an empty, listable staging directory was refused: {error}"),
    }
}
