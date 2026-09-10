//! The bound on a directory open macOS may hold behind a consent dialog.
//!
//! The dialog itself is never raised here: raising it is the defect. The
//! product's probe is driven with a real directory open that answers late,
//! which is exactly what a pending dialog looks like to the caller.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use stado::providers::local::disk_cleanup::consent::{self, Gated};
use stado::providers::local::disk_cleanup::safefs;

/// Longer than the probe's own bound, shorter than a test should take.
const LATE_ANSWER: Duration = Duration::from_secs(7);

fn folder(case: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("consent")
        .join(format!("{case}-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("create the gated folder");
    root
}

#[test]
fn an_open_that_answers_late_is_pending_within_the_bound_and_confirmed_afterwards() {
    let gated = folder("late");
    let target = gated.clone();
    let started = Instant::now();
    let probe = consent::bounded(&gated, move || {
        std::thread::sleep(LATE_ANSWER);
        safefs::open_dir_path(&target)
    })
    .expect("the probe runs");
    assert!(
        matches!(probe, Gated::Pending),
        "a late answer was not reported pending"
    );
    assert!(
        started.elapsed() < LATE_ANSWER,
        "the caller waited for the answer instead of the bound"
    );

    // While the answer is outstanding, nothing under the folder is opened and
    // no second probe is started for it.
    let opens = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&opens);
    let again = consent::open_dir_path(std::slice::from_ref(&gated), &gated).expect("gate answers");
    assert!(matches!(again, Gated::Pending));
    let child = gated.join("child");
    std::fs::create_dir_all(&child).unwrap();
    let parent = safefs::open_dir_path(&gated).expect("the test itself may open the folder");
    let second = consent::bounded(&gated, move || {
        counted.fetch_add(1, Ordering::SeqCst);
        safefs::open_dir_path(&PathBuf::from("/"))
    })
    .expect("a probe of a pending folder answers");
    assert!(
        matches!(second, Gated::Pending) && opens.load(Ordering::SeqCst) == 0,
        "a second probe ran while the first was still waiting"
    );

    // Once the late answer arrives the folder is confirmed and every open is
    // direct: through the parent descriptor and by path alike.
    std::thread::sleep(LATE_ANSWER);
    let direct = consent::open_dir_at(
        std::slice::from_ref(&gated),
        std::os::fd::AsRawFd::as_raw_fd(&parent),
        OsStr::new("child"),
        &child,
    )
    .expect("the confirmed folder opens");
    assert!(matches!(direct, Gated::Opened(_)));
    let by_path = consent::open_dir_path(std::slice::from_ref(&gated), &child).expect("opens");
    assert!(matches!(by_path, Gated::Opened(_)));
    std::fs::remove_dir_all(&gated).unwrap();
}

#[test]
fn an_open_that_answers_in_time_is_confirmed_at_once_and_a_refused_one_pins_nothing() {
    let gated = folder("prompt");
    let target = gated.clone();
    let probe = consent::bounded(&gated, move || safefs::open_dir_path(&target)).unwrap();
    assert!(matches!(probe, Gated::Opened(_)));
    let again = consent::open_dir_path(std::slice::from_ref(&gated), &gated).unwrap();
    assert!(matches!(again, Gated::Opened(_)));

    let refused = folder("refused");
    let missing = refused.join("absent");
    let probe = consent::bounded(&refused, move || safefs::open_dir_path(&missing));
    assert!(probe.is_err(), "a refused open was reported as an answer");
    // Nothing is pending for a refused folder: the next open asks again and
    // reports its own error.
    let missing = refused.join("absent");
    let next = consent::open_dir_path(std::slice::from_ref(&refused), &missing);
    assert!(next.is_err());
    std::fs::remove_dir_all(&gated).unwrap();
    std::fs::remove_dir_all(&refused).unwrap();
}

/// A folder outside the gated set is opened directly, whatever the gate's
/// state: the bound is for consent-gated folders only.
#[test]
fn an_ungated_folder_is_never_bounded() {
    let plain = folder("plain");
    let opened = consent::open_dir_path(&[], &plain).unwrap();
    assert!(matches!(opened, Gated::Opened(_)));
    std::fs::remove_dir_all(&plain).unwrap();
}
