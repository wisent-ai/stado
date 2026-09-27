//! A product lifecycle lock records its holder, and a refused lock names it.
//!
//! Through the real binary in an isolated home: `stado product rollback`
//! takes the surface's lock and records its pid, command line and time before
//! it refuses for want of an installation. While that lock file is held, a
//! second rollback is refused naming the recorded holder, that its pid is no
//! longer running, and since when — the facts two sessions installing Stado
//! needed on 2026-09-27 and did not get from "another writer owns".

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use fs2::FileExt;

fn home(label: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("product-lock-{label}"));
    if root.exists() {
        fs::remove_dir_all(&root).unwrap();
    }
    fs::create_dir_all(&root).unwrap();
    root
}

fn rollback(home: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .env("HOME", home)
        .args(["product", "rollback", "tama", "--surface", "cli"])
        .output()
        .expect("run stado product rollback")
}

#[test]
fn a_refused_lock_names_its_recorded_holder() {
    let home = home("holder");
    let first = rollback(&home);
    let first_error = String::from_utf8_lossy(&first.stderr);
    assert!(
        !first.status.success() && first_error.contains("no recorded installation"),
        "{first:?}"
    );

    let lock = home.join(".stado/products/tama/cli.lock");
    let record: serde_json::Value =
        serde_json::from_str(fs::read_to_string(&lock).unwrap().trim()).expect("a holder record");
    let pid = record["pid"].as_u64().expect("the holder's pid");
    assert!(
        record["command"]
            .as_str()
            .unwrap()
            .ends_with("product rollback tama --surface cli"),
        "{record}"
    );
    let since = record["acquired_at"]
        .as_str()
        .expect("when it was taken")
        .to_string();

    let held = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock)
        .unwrap();
    held.lock_exclusive().unwrap();
    let refused = rollback(&home);
    let message = String::from_utf8_lossy(&refused.stderr);
    assert!(!refused.status.success(), "{refused:?}");
    assert!(
        message.contains(&format!(
            "another writer owns {}; held by pid {pid} (no longer running) since {since}:",
            lock.display()
        )),
        "{message}"
    );
    assert!(
        message.contains("product rollback tama --surface cli"),
        "{message}"
    );
    assert!(
        !message.contains("no recorded installation"),
        "the second rollback ran past the lock: {message}"
    );
    held.unlock().unwrap();
}
