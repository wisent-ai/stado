//! Public release-channel contract through the real production ingress.
//!
//! This is deliberately ignored by a plain `cargo test`: it needs the public
//! control origin and one immutable release coordinate. Probierz supplies both
//! and retains the process output. No stand-in server, loopback override, dry
//! run, or fixture replaces the channel. The built Stado binary performs every
//! network operation; the test then verifies and executes the bytes it fetched.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use flate2::read::GzDecoder;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tar::Archive;

const PRODUCT: &str = "stado";

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is required by the public channel journey"))
}

fn stado(home: &Path, origin: &str, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_stado"));
    command
        .env_clear()
        .env("HOME", home)
        .env("PATH", std::env::var("PATH").expect("PATH exists"))
        .env("STADO_CONFIG", home.join("nonexistent-config.json"))
        .env("STADO_API_URL", origin)
        .args(args);
    if let Ok(token) = std::env::var("STADO_RELEASE_CHANNEL_TOKEN") {
        command.env("STADO_API_TOKEN", token);
    }
    command.output().expect("the built stado binary starts")
}

fn successful(out: Output, operation: &str) -> Vec<u8> {
    assert!(
        out.status.success(),
        "{operation} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    out.stdout
}

fn uri(version: &str, platform: &str, object: &str) -> String {
    format!("stado://releases/{PRODUCT}/{version}/{platform}/{object}")
}

fn get(home: &Path, origin: &str, object_uri: &str, destination: &Path) {
    let destination = destination.to_str().expect("temporary path is UTF-8");
    successful(
        stado(home, origin, &["storage", "get", object_uri, destination]),
        &format!("storage get {object_uri}"),
    );
}

fn manifest(path: &Path, version: &str, platform: &str) -> Value {
    let value: Value = serde_json::from_slice(&fs::read(path).expect("manifest was downloaded"))
        .expect("release manifest is JSON");
    assert_eq!(value["product"], PRODUCT);
    assert_eq!(value["version"], version);
    assert_eq!(value["platform"], platform);
    let digest = value["sha256"].as_str().expect("manifest sha256 is a string");
    assert_eq!(digest.len(), 64, "manifest sha256 has 64 hex digits");
    assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let source_commit = value["source_commit"]
        .as_str()
        .expect("manifest source_commit is a string");
    assert!(matches!(source_commit.len(), 40 | 64));
    assert!(source_commit.bytes().all(|byte| byte.is_ascii_hexdigit()));
    value
}

fn release_binary(archive: &Path, destination: &Path) -> PathBuf {
    Archive::new(GzDecoder::new(fs::File::open(archive).expect("release archive opens")))
        .unpack(destination)
        .expect("verified release archive extracts");
    let binary = destination.join("stado");
    assert!(binary.is_file(), "release archive contains the stado binary");
    binary
}

#[test]
#[ignore = "Probierz supplies and records the real public release channel"]
fn public_release_channel_serves_a_verified_executable_native_release() {
    let origin = required("STADO_RELEASE_CHANNEL_URL");
    assert!(
        origin.starts_with("https://") && !origin.contains("localhost") && !origin.contains("127.0.0.1"),
        "the release-channel journey must use public HTTPS, got {origin}",
    );
    let version = required("STADO_RELEASE_CHANNEL_VERSION");
    let platform = required("STADO_RELEASE_CHANNEL_PLATFORM");
    let work = tempfile::tempdir().expect("temporary test root");
    let home = work.path().join("home");
    fs::create_dir_all(&home).expect("temporary HOME exists");

    let manifest_name = format!("release-manifest-{platform}.json");
    let manifest_uri = uri(&version, &platform, &manifest_name);
    let stat = successful(
        stado(&home, &origin, &["storage", "stat", &manifest_uri, "--json"]),
        &format!("storage stat {manifest_uri}"),
    );
    let presence: Value = serde_json::from_slice(&stat).expect("storage stat emits JSON");
    assert_eq!(
        presence["state"], "present",
        "the public channel must testify that the manifest is present: {presence}",
    );

    let manifest_path = work.path().join(&manifest_name);
    get(&home, &origin, &manifest_uri, &manifest_path);
    let manifest = manifest(&manifest_path, &version, &platform);

    let archive_name = format!("stado-v{version}-{platform}.tar.gz");
    let archive_uri = uri(&version, &platform, &archive_name);
    let archive_path = work.path().join(&archive_name);
    get(&home, &origin, &archive_uri, &archive_path);
    let archive_bytes = fs::read(&archive_path).expect("release archive was downloaded");
    let actual_digest = hex::encode(Sha256::digest(&archive_bytes));
    assert_eq!(
        actual_digest,
        manifest["sha256"].as_str().unwrap(),
        "the public archive bytes match the signed release manifest",
    );

    let extracted = work.path().join("extracted");
    fs::create_dir(&extracted).expect("extract directory exists");
    let released_stado = release_binary(&archive_path, &extracted);
    let version_out = Command::new(&released_stado)
        .arg("--version")
        .output()
        .expect("the released native binary executes");
    assert!(
        version_out.status.success(),
        "released binary --version failed: {}",
        String::from_utf8_lossy(&version_out.stderr),
    );
    let reported = String::from_utf8(version_out.stdout).expect("version output is UTF-8");
    assert!(
        reported.contains(&version),
        "released binary reports {reported:?}, expected version {version}",
    );
    println!(
        "verified {manifest_uri}; archive={archive_uri}; sha256={actual_digest}; binary={}",
        released_stado.display(),
    );
}
