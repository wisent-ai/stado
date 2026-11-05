//! Backup copies of installed binaries under `~/.stado/bin`: at the
//! disk-full rule a pass removes every backup of a binary but its newest,
//! and keeps the binary, its `.previous` rollback target, its coordinate and
//! names of any other shape. Run on a real APFS volume under the
//! repository's build directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::native::Native;

/// A file of `bytes` at `bin/name`, its modification time `age` seconds ago.
fn binary(bin: &Path, name: &str, age: u64) -> PathBuf {
    let path = bin.join(name);
    fs::write(&path, vec![b'b'; 64 * 1024]).unwrap();
    fs::File::open(&path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(age))
        .unwrap();
    path
}

#[test]
fn at_the_threshold_every_backup_but_the_newest_goes() {
    let native = Native::new("bin-backups");
    let bin = native.home.join(".stado/bin");
    fs::create_dir_all(&bin).unwrap();
    let installed = binary(&bin, "widget", 0);
    let previous = binary(&bin, "widget.previous", 50);
    let coordinate = binary(&bin, "widget.release-version", 0);
    let newest = binary(&bin, "widget.release-backup-20261103", 100);
    let older = binary(&bin, "widget.release-backup-20261001", 200);
    let oldest = binary(&bin, "widget.0.23.1-backup-a", 300);
    let pre = binary(&bin, "widget.pre-upgrade", 400);
    let foreign = binary(&bin, "widget.copy", 500);
    let user_data = native.fill_with_user_data();

    let report = native.cleanup();
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    let cleaner = &report["cleaners"]["bin_backups"];
    assert_eq!(cleaner["scanned_items"], 4, "{report}");
    assert_eq!(cleaner["deleted_items"], 3, "{report}");
    assert_eq!(cleaner["skipped"]["newest_backup"], 1, "{report}");
    for kept in [&installed, &previous, &coordinate, &newest, &foreign] {
        assert!(kept.is_file(), "{} was taken: {report}", kept.display());
    }
    for taken in [&older, &oldest, &pre] {
        assert!(
            !taken.exists(),
            "{} survived the rule: {report}",
            taken.display()
        );
    }
    assert!(user_data.is_file(), "the user's data was deleted");
}
