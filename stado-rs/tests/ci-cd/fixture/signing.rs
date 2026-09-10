//! The pinned native signer a darwin build signs with, read from the fleet's
//! object namespace and placed in the isolated one.
//!
//! The worker resolves the signer from `artifacts/native-signing/<sha>.tar.gz`
//! in its own namespace and installs it into the isolated home's Stado cache
//! through the real runtime payload; nothing on PATH is consulted. The input
//! is immutable and addressed by its digest, so the copy is the same bytes
//! the fleet builds sign with, and it is written through the isolated
//! profile's own `storage put` so the layout is the product's, not a guess.

use super::*;

/// Place the fleet's pinned signing input where the isolated worker reads
/// it, or say which fleet read stood in the way.
pub(crate) fn seed_native_signing_input(home: &Path, storage: &Path) {
    let sha = stado::deploy::native_signing::SIGNER_SOURCE_SHA256;
    let leaf = format!("{sha}.tar.gz");
    let fetched_copy = home.join("native-signing-input.tar.gz");
    let fleet_namespace = std::env::var("WC_STADO_STORAGE_NAMESPACE").unwrap_or_default();
    let fleet_namespace = if fleet_namespace.is_empty() {
        "probierz".to_owned()
    } else {
        fleet_namespace
    };
    let uri = format!("stado://{fleet_namespace}/artifacts/native-signing/{leaf}");
    let fetched = Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["storage", "get", &uri])
        .arg(&fetched_copy)
        .output()
        .expect("the Stado CLI runs");
    assert!(
        fetched.status.success(),
        "blocked: the pinned native signing input {uri} could not be read from the fleet store: {}",
        String::from_utf8_lossy(&fetched.stderr)
    );
    let bytes = fs::read(&fetched_copy).unwrap();
    let digest = {
        use sha2::Digest;
        hex::encode(sha2::Sha256::digest(&bytes))
    };
    assert_eq!(
        digest, sha,
        "the fleet store served a signing input that is not the pinned one"
    );
    run(Command::new(env!("CARGO_BIN_EXE_stado"))
        .env_clear()
        .env("HOME", home)
        .env("PATH", std::env::var("PATH").unwrap())
        .env("STADO_CONFIG", home.join(".stado/config.json"))
        .env("WC_STORAGE_BACKEND", "local")
        .env("WC_LOCAL_STORAGE_PATH", storage)
        .env("WC_STADO_STORAGE_NAMESPACE", "ci-release")
        .args([
            "storage",
            "put",
            "--content-type",
            "application/gzip",
            &format!("stado://ci-release/artifacts/native-signing/{leaf}"),
        ])
        .arg(&fetched_copy));
}
