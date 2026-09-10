//! The native signer a darwin release job signs with, resolved on the builder
//! itself.
//!
//! On 2026-09-10 the release worker on charless-mac-mini looked `wisent-products`
//! up on PATH, found nothing, and weles-worker 0.6.6 died at `macos-code-signing`
//! with "cannot run wisent-products: No such file or directory" - on a host the
//! GUI-automation installer had provisioned with the pinned signer the day
//! before. The worker now resolves the same pinned revision from the same
//! Stado-owned cache. Two facts are measured here, against the real resolver
//! with an isolated home under this crate's own build directory: a pinned
//! signer already in the cache is returned as-is, and a missing one with no
//! readable source is refused by name rather than replaced by a PATH lookup.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use stado::deploy::native_signing::{bootstrap_local_signer, signer_program, SIGNER_SOURCE_SHA256};
use stado::deploy::production_runner;

/// An isolated home for one case, under the build directory Cargo hands
/// integration tests, so no operator file is read or written.
fn isolated_home(case: &str) -> PathBuf {
    let home = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("native-signing")
        .join(format!("{case}-{}", std::process::id()));
    if home.exists() {
        fs::remove_dir_all(&home).expect("clear the previous fixture");
    }
    fs::create_dir_all(&home).expect("create the fixture home");
    home
}

#[tokio::test]
async fn a_pinned_signer_already_in_the_cache_is_returned_without_a_fetch() {
    let home = isolated_home("present");
    let program = PathBuf::from(signer_program(home.to_str().unwrap()));
    fs::create_dir_all(program.parent().unwrap()).unwrap();
    fs::write(&program, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(program.to_str().unwrap().contains(SIGNER_SOURCE_SHA256));

    std::env::set_var("HOME", &home);
    let resolved = bootstrap_local_signer(&production_runner())
        .await
        .expect("the cached signer resolves");
    assert_eq!(resolved, program.to_str().unwrap());

    fs::remove_dir_all(&home).unwrap();
}

#[tokio::test]
async fn a_missing_signer_without_a_readable_source_is_refused_by_name() {
    let home = isolated_home("absent");
    std::env::set_var("HOME", &home);
    // No pinned source can be read from an isolated home: the object client
    // has no credential there. The refusal must say which input it could not
    // read, and the PATH is never consulted in its place.
    let refused = bootstrap_local_signer(&production_runner())
        .await
        .expect_err("an absent signer with no source is refused");
    let message = refused.to_string();
    assert!(
        message.contains("native signing input") || message.contains("storage.stado.namespace"),
        "the refusal does not name the missing source: {message}"
    );
    assert!(!PathBuf::from(signer_program(home.to_str().unwrap())).exists());

    fs::remove_dir_all(&home).unwrap();
}
