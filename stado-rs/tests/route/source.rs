//! Real current and historical brokers, without copying another checkout.
//! Current builds use the one canonical Skarbiec checkout. The historical
//! dependency is downloaded from its immutable release and digest-checked.

use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use sha2::{Digest, Sha256};

const HISTORICAL_VERSION: &str = "0.2.39";
const HISTORICAL_REVISION: &str = "33daa68378c4fb81b12f3a108ccd4906204cc440";
const HASH_BUFFER_BYTES: usize = 8192;
static BROKER: LazyLock<PathBuf> = LazyLock::new(resolve);
static HISTORICAL_BROKER: LazyLock<PathBuf> = LazyLock::new(download_historical);

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
            "SKARBIEC_BIN is not executable: {}",
            binary.display()
        );
        return binary;
    }
    let repo = skarbiec_repo();
    run(
        Command::new("cargo")
            .args(["build", "--locked", "--release", "--bin", "skarbiec"])
            .current_dir(&repo)
            .env("CARGO_TARGET_DIR", repo.join("target"))
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .env_remove("RUSTFLAGS"),
        "build the canonical Skarbiec checkout",
    );
    let binary = repo.join("target/release/skarbiec");
    assert!(
        executable(&binary),
        "build produced no broker: {}",
        binary.display()
    );
    binary
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
            .args(["storage", "get", uri]).arg(destination),
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
        "no canonical Skarbiec checkout at {}; set SKARBIEC_REPO or SKARBIEC_BIN",
        repo.display()
    );
    repo
}

fn git(directory: &Path, arguments: &[&str]) -> String {
    String::from_utf8(run(
        Command::new("git").arg("-C").arg(directory).args(arguments),
        "read the canonical checkout",
    ))
    .expect("git answers in UTF-8")
    .trim()
    .to_owned()
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
