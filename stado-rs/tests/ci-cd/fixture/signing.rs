//! The pinned native-signing inputs a darwin build signs with, read from the
//! fleet's object namespace and placed in the isolated one.
//!
//! The worker resolves two immutable inputs from `artifacts/native-signing/`
//! in its own namespace: the signer `<sha>.tar.gz`, installed into the
//! isolated home's Stado cache through the real runtime payload, and the
//! Apple issuer chain `apple-issuers-<sha>.pem` the signing step verifies
//! the identity against. Nothing on PATH is consulted. Each input is
//! addressed by its digest, so the copy is the same bytes the fleet builds
//! sign with, and it is written through the isolated profile's own
//! `storage put` so the layout is the product's, not a guess.
//!
//! Until 2026-09-18 only the signer was seeded; every darwin journey then
//! died in `macos-code-signing` with `cannot read native signing input
//! .../apple-issuers-<sha>.pem: absent`, and the journey never reached
//! publication. A real release needs both, so the fixture stages both.

use super::*;

/// Place the fleet's pinned signing inputs where the isolated worker reads
/// them, or say which fleet read stood in the way.
pub(crate) fn seed_native_signing_input(home: &Path, storage: &Path) {
    let signer = stado::deploy::native_signing::SIGNER_SOURCE_SHA256;
    let issuers = stado::deploy::native_signing::APPLE_ISSUER_CHAIN_SHA256;
    for (leaf, sha, content_type) in [
        (format!("{signer}.tar.gz"), signer, "application/gzip"),
        (
            format!("apple-issuers-{issuers}.pem"),
            issuers,
            "application/x-pem-file",
        ),
    ] {
        seed_pinned_input(home, storage, &leaf, sha, content_type);
    }
}

fn seed_pinned_input(home: &Path, storage: &Path, leaf: &str, sha: &str, content_type: &str) {
    let fetched_copy = home.join(format!("native-signing-input-{leaf}"));
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
            content_type,
            &format!("stado://ci-release/artifacts/native-signing/{leaf}"),
        ])
        .arg(&fetched_copy));
}
