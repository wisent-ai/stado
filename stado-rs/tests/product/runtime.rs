use crate::fixture::{stderr, Journey};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

fn archived_executable_digest(path: &Path) -> String {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(File::open(path).unwrap()));
    let mut digest = None;
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if entry.path().unwrap() != Path::new("wisent-products") {
            continue;
        }
        assert!(
            digest.is_none(),
            "the actual SDK archive contains an ambiguous executable"
        );
        assert!(
            entry.header().entry_type().is_file(),
            "the qualified SDK entry is not a regular executable"
        );
        let mut hash = Sha256::new();
        let mut buffer = [0_u8; 65536];
        loop {
            let read = entry.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            hash.update(&buffer[..read]);
        }
        digest = Some(hex::encode(hash.finalize()));
    }
    digest.expect("the real qualified SDK archive omitted its executable")
}

#[test]
fn qualified_sdk_bytes_and_source_claim_cannot_drift() {
    let journey = Journey::new();
    let program = journey.sdk_path();
    let record = program.parent().unwrap().join("sdk-receipt.json");
    let original = fs::read(&record).unwrap();
    journey.retain("sdk-receipt.json", &original);
    let receipt: Value = serde_json::from_slice(&original).unwrap();
    let source = receipt["artifact"]["source_revision"].as_str().unwrap();
    let platform = receipt["platform"].as_str().unwrap();
    let archive = journey.home.join("independently-fetched-sdk.tar.gz");
    let fetched = journey.stado(&[
        "release",
        "fetch",
        "wisent-products",
        stado::deploy::native_signing::runtime::VERSION,
        "--platform",
        platform,
        "--source-commit",
        source,
        "--destination",
        archive.to_str().unwrap(),
        "--json",
    ]);
    assert!(
        fetched.status.success(),
        "the native SDK cannot be independently verified against its signed release: {}",
        stderr(&fetched)
    );
    let (_, actual) = stado::release_control::sha256_file(&program).unwrap();
    assert_eq!(
        actual,
        archived_executable_digest(&archive),
        "Stado did not install the qualified SDK's actual executable bytes"
    );

    let mut changed = receipt.clone();
    let wrong_source = "0".repeat(source.len());
    assert_ne!(
        source, wrong_source,
        "the negative source claim must differ from the actual release"
    );
    changed["artifact"]["source_revision"] = Value::String(wrong_source.clone());
    fs::write(&record, serde_json::to_vec_pretty(&changed).unwrap()).unwrap();
    let refused = journey.stado(&["product", "catalog", "--json"]);
    assert!(
        !refused.status.success(),
        "a declared source replaced executable provenance"
    );
    let diagnostic = stderr(&refused);
    assert!(
        diagnostic.contains(source) && diagnostic.contains(&wrong_source),
        "the source refusal omitted the expected or observed source: {diagnostic}"
    );
    assert_eq!(
        stado::release_control::sha256_file(&program).unwrap().1,
        actual,
        "a refused source claim changed the installed native SDK"
    );
    fs::write(&record, &original).unwrap();

    OpenOptions::new()
        .append(true)
        .open(&program)
        .unwrap()
        .write_all(b"changed-sdk-bytes")
        .unwrap();
    let (_, changed_digest) = stado::release_control::sha256_file(&program).unwrap();
    assert_ne!(changed_digest, actual);
    let refused = journey.stado(&["product", "catalog", "--json"]);
    assert!(
        !refused.status.success(),
        "the product command succeeded after the managed SDK bytes changed"
    );
    let diagnostic = stderr(&refused);
    assert!(
        diagnostic.contains(&actual) && diagnostic.contains(&changed_digest),
        "the byte refusal omitted expected and actual executable digests: {diagnostic}"
    );
    journey.assert_no_installation();
    journey.finish();
}
