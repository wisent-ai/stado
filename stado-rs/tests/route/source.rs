//! The current Skarbiec broker this area resolves declared routes through.
//!
//! No stand-in on PATH and no canned answer, and no environment variable an
//! operator has to remember: the binary is either the one named in
//! `SKARBIEC_BIN`, or the one built here from the sibling Skarbiec checkout at
//! `origin/main`. The source is taken with `git archive` into an ignored cache
//! keyed by commit, so the operator's own working tree — which routinely
//! carries uncommitted work, and at the time of writing does not compile — is
//! read and never touched, and a second run reuses the build.
//!
//! Same shape as `tests/credentials_host/broker.rs`, because both areas need
//! the same real dependency and two ways of getting it would drift.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

const CACHE: &str = "../.wisent-output/route-skarbiec";
const OWNER_ONLY_EXECUTABLE: u32 = 0o700;
// This released source predates the route group; the current host's install
// cannot serve as the fixture because upgrading it would break this test.
const HISTORICAL_REVISION: &str = "33daa68378c4fb81b12f3a108ccd4906204cc440";

/// One build per test binary. Dereferencing the lock blocks the other test
/// threads while the first one exports and compiles, so parallel cases cannot
/// race each other through the same export directory.
static BROKER: LazyLock<PathBuf> = LazyLock::new(resolve);
static HISTORICAL_BROKER: LazyLock<PathBuf> =
    LazyLock::new(|| cached_build(&skarbiec_repo(), HISTORICAL_REVISION));

pub fn real_skarbiec() -> PathBuf {
    BROKER.clone()
}

pub fn historical_skarbiec() -> PathBuf {
    HISTORICAL_BROKER.clone()
}

fn resolve() -> PathBuf {
    if let Some(named) = std::env::var_os("SKARBIEC_BIN") {
        let binary = PathBuf::from(named);
        assert!(
            executable(&binary),
            "blocked: SKARBIEC_BIN does not name an executable file: {}",
            binary.display()
        );
        return binary;
    }
    let repo = skarbiec_repo();
    let commit = git(&repo, &["rev-parse", "origin/main"]);
    cached_build(&repo, &commit)
}

fn cached_build(repo: &Path, commit: &str) -> PathBuf {
    let cache = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(CACHE)
        .join(commit);
    let binary = cache.join("skarbiec");
    if executable(&binary) {
        return binary;
    }
    build(repo, commit, &cache);
    assert!(
        executable(&binary),
        "blocked: the Skarbiec build produced no executable at {}",
        binary.display()
    );
    binary
}

/// The Skarbiec checkout beside this one.
///
/// Resolved from the common git directory rather than from
/// `CARGO_MANIFEST_DIR`, because this area is expected to run from a linked
/// worktree as well as from the primary checkout, and only the common
/// directory names the place both of them were cloned into.
fn skarbiec_repo() -> PathBuf {
    if let Some(named) = std::env::var_os("SKARBIEC_REPO") {
        return PathBuf::from(named);
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let common = PathBuf::from(git(
        manifest,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ));
    let repo = common
        .parent()
        .and_then(Path::parent)
        .map(|siblings| siblings.join("skarbiec"))
        .unwrap_or_default();
    assert!(
        repo.join(".git").exists(),
        "blocked: no Skarbiec checkout at {}; name one in SKARBIEC_REPO or a built binary in \
         SKARBIEC_BIN. This area resolves declared routes through the real broker and does not \
         pretend to without one.",
        repo.display()
    );
    repo
}

fn build(repo: &Path, commit: &str, cache: &Path) {
    let source = cache.join("source");
    let archive = cache.join("source.tar");
    fs::create_dir_all(&source).expect("create the Skarbiec export directory");
    run(
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["archive", "--format=tar", "-o"])
            .arg(&archive)
            .arg(commit),
        "export the selected immutable Skarbiec revision",
    );
    run(
        Command::new("tar")
            .arg("-xf")
            .arg(&archive)
            .arg("-C")
            .arg(&source),
        "unpack the Skarbiec export",
    );
    run(
        Command::new("cargo")
            .args(["build", "--locked", "--release", "--bin", "skarbiec"])
            .current_dir(&source)
            .env("CARGO_TARGET_DIR", cache.join("target"))
            .env_remove("CARGO")
            .env_remove("CARGO_MAKEFLAGS")
            .env_remove("CARGO_MANIFEST_DIR")
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env_remove("RUSTFLAGS")
            .env_remove("RUSTC")
            .env_remove("RUSTC_WRAPPER")
            .env_remove("RUSTDOC"),
        "build the real Skarbiec broker",
    );
    let built = cache.join("target/release/skarbiec");
    fs::copy(&built, cache.join("skarbiec")).expect("keep the built broker beside its export");
    fs::set_permissions(
        cache.join("skarbiec"),
        fs::Permissions::from_mode(OWNER_ONLY_EXECUTABLE),
    )
    .expect("make the built broker owner-only executable");
}

fn git(directory: &Path, arguments: &[&str]) -> String {
    let output = run(
        Command::new("git").arg("-C").arg(directory).args(arguments),
        "read the git checkout",
    );
    String::from_utf8(output)
        .expect("git answers in UTF-8")
        .trim()
        .to_string()
}

fn run(command: &mut Command, purpose: &str) -> Vec<u8> {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("blocked: could not {purpose}: {error}"));
    assert!(
        output.status.success(),
        "blocked: could not {purpose}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    output.stdout
}

fn executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|data| data.is_file() && data.permissions().mode() & 0o111 != 0)
}
