//! A product build holds the janitor's cleanup lock, as every agent job does,
//! and waits for a cleanup pass instead of being refused by it: the real
//! `stado product install` in the isolated home, started while a pass holds
//! that lock exclusively, says on stderr that it waits for the janitor's
//! cleanup pass before anything is exported or built, and once the pass lets
//! go it takes its hold and gets past it (and stops later, on what the
//! isolated home does not hold, which is not the janitor).
//!
//! A product build is refused while the janitor has asked for its turn,
//! unless it was asked to `--wait`: then it takes that turn itself. With a
//! standing turn request in the isolated home, the real install without
//! `--wait` is refused naming the request and leaves it standing; with
//! `--wait` it runs the janitor's pass, the request is answered, and the
//! install gets past the janitor.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::os::fd::AsRawFd;
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};

use super::native::Native;

const INSTALL: &[&str] = &["product", "install", "tama", "--surface", "cli"];

#[test]
fn a_product_install_waits_for_the_cleanup_pass_and_runs_once_it_lets_go() {
    let native = Native::new("product-hold");
    fs::create_dir_all(native.state_dir()).unwrap();
    let lock = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(native.state_dir().join("disk-cleanup.lock"))
        .unwrap();
    // SAFETY: a descriptor this test owns; the lock is released when `lock`
    // is dropped below.
    let locked =
        unsafe { nix::libc::flock(lock.as_raw_fd(), nix::libc::LOCK_EX | nix::libc::LOCK_NB) };
    assert!(
        !locked.is_negative(),
        "the test could not hold the cleanup lock"
    );

    let mut install = native
        .command(INSTALL)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start the real install");
    let mut stderr = BufReader::new(install.stderr.take().expect("install stderr"));
    let mut said = String::new();
    // Read until the install says it waits for the pass, or ends without.
    let waiting = loop {
        let mut line = String::new();
        stderr.read_line(&mut line).expect("read install stderr");
        said.push_str(&line);
        if line.is_empty() {
            break None;
        }
        if line.starts_with("czekam:") && line.contains("cleanup pass") {
            break Some(line);
        }
    };
    native.observe(
        "install while a pass holds the lock",
        serde_json::json!({ "waiting": waiting, "stderr": said }),
    );
    assert!(
        waiting.is_some(),
        "the install did not wait for the pass: {said}"
    );
    assert!(!said.contains("nothing was exported or built"), "{said}");

    drop(lock);
    stderr
        .read_to_string(&mut said)
        .expect("read install stderr");
    let status = install.wait().expect("the install ends");
    native.observe(
        "install once the pass let go",
        serde_json::json!({ "status": status.code(), "stderr": said }),
    );
    assert!(
        said.contains("koniec czekania"),
        "the wait for the pass never ended: {said}"
    );
    assert!(!said.contains("disk janitor reclaims"), "{said}");
}

#[test]
fn a_product_install_is_refused_a_janitor_turn_unless_it_waits_and_then_takes_it() {
    let native = Native::new("product-turn");
    fs::create_dir_all(native.state_dir()).unwrap();
    let turn = native.state_dir().join("disk-cleanup-turn.json");
    let requested_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock reads after the Unix epoch")
        .as_secs_f64();
    // The request stands behind this test's process, which lives for the
    // whole journey, so neither install reads it as abandoned.
    let request = serde_json::json!({
        "requested_at": requested_at,
        "reason": "the product-turn journey asks for the janitor's turn",
        "pid": std::process::id(),
    });
    fs::write(&turn, request.to_string()).unwrap();

    let refused = native
        .command(INSTALL)
        .stdout(Stdio::null())
        .output()
        .expect("run the real install");
    let said = String::from_utf8_lossy(&refused.stderr).into_owned();
    native.observe(
        "install without --wait while the janitor asks for its turn",
        serde_json::json!({ "status": refused.status.code(), "stderr": said }),
    );
    assert!(!refused.status.success(), "{said}");
    assert!(
        said.contains("has asked running work for its turn"),
        "{said}"
    );
    assert!(said.contains("janitor turn requested_at"), "{said}");
    assert!(said.contains("nothing was exported or built"), "{said}");
    assert!(
        turn.is_file(),
        "a refused install answered the turn: {said}"
    );

    let mut waiting = INSTALL.to_vec();
    waiting.push("--wait");
    let waited = native
        .command(&waiting)
        .stdout(Stdio::null())
        .output()
        .expect("run the real install with --wait");
    let said = String::from_utf8_lossy(&waited.stderr).into_owned();
    native.observe(
        "install with --wait while the janitor asks for its turn",
        serde_json::json!({ "status": waited.status.code(), "stderr": said }),
    );
    assert!(
        !said.contains("has asked running work for its turn"),
        "{said}"
    );
    assert!(!said.contains("nothing was exported or built"), "{said}");
    assert!(!said.contains("disk janitor reclaims"), "{said}");
    assert!(
        !turn.exists(),
        "the pass the waiting install ran did not answer the turn: {said}"
    );
}
