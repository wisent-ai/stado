// Shared by the test targets that need a broker predating a verb group.
#![allow(dead_code)]

//! The one older broker a delivery-gap case can be proved against.
//!
//! Which Skarbiec the fleet runs is decided by `support/skarbiec.rs` and
//! nothing here: this file answers a different question, and only the
//! question a current binary cannot answer — what Stado says when a host
//! still runs a broker that predates a command group. That binary is not on
//! a developer machine or a fresh CI runner, so it is read from its own
//! immutable release by version, source revision and archive digest. Pinned
//! in code, verified before use, and never a build of another checkout.

use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use sha2::{Digest, Sha256};

/// The release this fixture is pinned to: the last Skarbiec published before
/// the `route` verb group existed.
const HISTORICAL_VERSION: &str = "0.2.39";
const HISTORICAL_REVISION: &str = "33daa68378c4fb81b12f3a108ccd4906204cc440";
const HASH_BUFFER_BYTES: usize = 8192;
static HISTORICAL_BROKER: LazyLock<PathBuf> = LazyLock::new(download_historical);

/// The verified historical broker, downloaded once per test binary.
pub fn historical_skarbiec() -> PathBuf {
    HISTORICAL_BROKER.clone()
}

fn download_historical() -> PathBuf {
    let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("linux", "x86_64") => "linux-amd64",
        other => panic!("no historical release platform for {other:?}; provide SKARBIEC_STALE_BIN"),
    };
    let cache = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/test-dependencies/skarbiec")
        .join(format!("{HISTORICAL_VERSION}-{platform}"));
    fs::create_dir_all(&cache).expect("create the test dependency cache");
    let prefix = format!("stado://releases/skarbiec/{HISTORICAL_VERSION}/{platform}");
    let manifest = cache.join("release.json");
    download(&format!("{prefix}/release.json"), &manifest);
    let identity: serde_json::Value = serde_json::from_slice(&fs::read(&manifest).unwrap())
        .expect("historical release manifest is JSON");
    assert_eq!(identity["source_revision"], HISTORICAL_REVISION);
    assert_eq!(identity["binary"], "bin/skarbiec");
    let expected = identity["artifact_sha256"]
        .as_str()
        .expect("archive digest");
    let archive = cache.join("release.tar.gz");
    if digest(&archive).as_deref() != Some(expected) {
        download(&format!("{prefix}/release.tar.gz"), &archive);
    }
    assert_eq!(
        digest(&archive).as_deref(),
        Some(expected),
        "historical archive digest mismatch"
    );
    run(
        Command::new("tar")
            .arg("-xzf")
            .arg(&archive)
            .arg("-C")
            .arg(&cache)
            .arg("bin/skarbiec"),
        "extract the verified historical broker",
    );
    let binary = cache.join("bin/skarbiec");
    assert!(
        executable(&binary),
        "archive did not contain an executable broker"
    );
    binary
}

fn download(uri: &str, destination: &Path) {
    run(
        Command::new(env!("CARGO_BIN_EXE_stado"))
            .args(["storage", "get", uri])
            .arg(destination),
        "read the real historical release; provide SKARBIEC_STALE_BIN if this platform was not published",
    );
}

fn digest(path: &Path) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let mut hash = Sha256::new();
    let mut buffer = [0; HASH_BUFFER_BYTES];
    loop {
        let size = file.read(&mut buffer).ok()?;
        if size == 0 {
            break;
        }
        hash.update(&buffer[..size]);
    }
    Some(format!("{:x}", hash.finalize()))
}

fn run(command: &mut Command, purpose: &str) -> Vec<u8> {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("blocked: could not {purpose}: {error}"));
    assert!(
        output.status.success(),
        "blocked: could not {purpose}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|data| data.is_file() && data.permissions().mode() & 0o111 != 0)
}
