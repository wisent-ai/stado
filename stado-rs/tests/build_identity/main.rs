//! `stado --version` must say which tree the binary came from.
//!
//! The contract this defends: an operator asking a host what it is running
//! gets an answer, without dissecting a binary. On 2026-09-03 the version
//! `0.14.6` named four materially different trees of this crate — the one the
//! fleet was running (lacking the janitor workload-hold fix and the builder
//! claimability fix), two separate commits that each declared `0.14.6`, and a
//! local build with a fourth combination — and no release object existed to
//! tell them apart. Establishing what was actually deployed took `strings` and
//! `nm` against the installed binary. This test is why that is no longer the
//! way to find out.
//!
//! It drives the real binary rather than reading the constant, because the
//! constant being right is not the contract: the contract is that the shipped
//! executable prints it.

use std::process::Command;

/// The executable cargo just built for this test run.
const STADO: &str = env!("CARGO_BIN_EXE_stado");

/// The crate version the test binary was compiled against. The binary under
/// test is built from the same tree in the same cargo invocation.
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// `--version` as the shipped binary prints it.
fn version_line() -> String {
    let output = Command::new(STADO)
        .arg("--version")
        .output()
        .unwrap_or_else(|error| panic!("{STADO} --version did not run: {error}"));
    assert!(
        output.status.success(),
        "{STADO} --version exited {:?}; stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("--version is utf-8")
        .trim()
        .to_string()
}

#[test]
fn the_version_line_names_the_semantic_version() {
    let printed = version_line();
    assert!(
        printed.contains(VERSION),
        "{printed:?} does not name version {VERSION}"
    );
}

/// The defect itself. Before the fix this line was `stado 0.14.6` and stopped
/// there, which is exactly the state that made four trees indistinguishable.
#[test]
fn the_version_line_names_the_tree_it_was_built_from() {
    let printed = version_line();
    let revision = printed
        .split_once("(rev ")
        .and_then(|(_, rest)| rest.split_once(')'))
        .map(|(revision, _)| revision.trim().to_string());
    let revision = revision.unwrap_or_else(|| {
        panic!(
            "{printed:?} carries no revision. The version alone does not identify \
             content: 0.14.6 named four different trees of this crate, and answering \
             'which build is this' meant reading symbols out of the binary."
        )
    });
    assert!(
        !revision.is_empty(),
        "{printed:?} names an empty revision, which answers nothing"
    );
    // Either a git revision, or the stated sentinel for a build context that
    // has no git metadata. A tarball build is legitimate and must still print
    // a usable line.
    if revision == "unknown" {
        return;
    }
    let core = revision.strip_suffix("-dirty").unwrap_or(&revision);
    assert!(
        core.len() >= 7 && core.len() <= 40,
        "{core:?} is not a git revision length"
    );
    assert!(
        core.chars().all(|character| character.is_ascii_hexdigit()),
        "{core:?} is not hexadecimal, so it is not a revision"
    );
}

/// One line, so it can be read out of a log or a `--version` capture without
/// anybody having to know how many lines to expect.
#[test]
fn the_version_line_is_one_line() {
    let printed = version_line();
    assert_eq!(
        printed.lines().count(),
        1,
        "--version printed {} lines: {printed:?}",
        printed.lines().count()
    );
}

/// A build in this repository, with git present, must name a real revision
/// rather than falling back to the sentinel. Skipped only where the fallback
/// is legitimate, which is a checkout with no git metadata at all.
#[test]
fn a_git_checkout_names_a_real_revision() {
    let git_available = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if !git_available {
        return;
    }
    let printed = version_line();
    assert!(
        !printed.contains("(rev unknown)"),
        "{printed:?} fell back to the sentinel inside a git checkout, so the build \
         script failed to read a revision it could have read"
    );
}
