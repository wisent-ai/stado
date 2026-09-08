//! What end-to-end integrity has to keep meaning: a whole payload verifies
//! byte-exact, a short or wrongly sized one is a mismatch that names both
//! digests, a refusal carries no bytes, the path survives the round trip, and
//! the operand never reaches the host in the clear.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

use super::*;
use crate::deploy::service_file_fetch::{FILE_REFUSED_SYMLINK, MAX_FETCH_BYTES};

fn read_report(bytes: &[u8]) -> FetchReport {
    FetchReport {
        path: "/Users/charles/.stado/bin/weles-release-cutover".to_string(),
        file_state: FILE_READ.to_string(),
        detail: String::new(),
        mode: "700".to_string(),
        owner_only: true,
        bytes: bytes.len() as u64,
        digest: digest_of(bytes),
        content_b64: STANDARD.encode(bytes),
    }
}

#[test]
fn a_whole_payload_verifies_and_decodes_byte_exact() {
    // The bytes `env-show` cannot report: a quote, a backslash, a tab and a
    // non-ASCII byte are exactly what its sanitizer replaces with `?`.
    let bytes = b"#!/bin/bash\nsed -E \"/^WC_SKARBIEC_URL=/d\" \\\n\t--\xc3\xa9\n";
    let fetched = verify(read_report(bytes));
    assert_eq!(fetched.integrity, INTEGRITY_VERIFIED);
    assert!(fetched.ok());
    assert_eq!(fetched.content, bytes);
    assert_eq!(fetched.failure("charless-mac-mini"), None);
}

#[test]
fn a_truncated_payload_is_a_mismatch_and_says_both_digests() {
    let bytes = b"#!/bin/bash\nexit 0\n";
    let mut report = read_report(bytes);
    report.content_b64 = STANDARD.encode(&bytes[..4]);
    let fetched = verify(report);
    assert_eq!(fetched.integrity, INTEGRITY_MISMATCH);
    assert!(!fetched.ok());
    let failure = fetched.failure("charless-mac-mini").unwrap();
    assert!(failure.contains(&fetched.local_digest), "{failure}");
    assert!(failure.contains("nothing was written"), "{failure}");
}

#[test]
fn a_payload_matching_its_digest_at_the_wrong_size_is_still_a_mismatch() {
    // The digest is the host's word about the file; `bytes` is `stat`'s.
    // Two host-side answers that disagree mean the file changed under the
    // read, and a fetch that reported `verified` there would hand the
    // caller bytes no single version of the file ever had.
    let bytes = b"one";
    let mut report = read_report(bytes);
    report.bytes = 99;
    assert_eq!(verify(report).integrity, INTEGRITY_MISMATCH);
}

#[test]
fn a_refusal_carries_no_bytes_and_is_never_verified() {
    let report = FetchReport {
        path: String::new(),
        file_state: FILE_REFUSED_SYMLINK.to_string(),
        detail: "the target is a symlink and was not followed".to_string(),
        mode: "unknown".to_string(),
        owner_only: false,
        bytes: 0,
        digest: String::new(),
        content_b64: String::new(),
    };
    let fetched = verify(report);
    assert_eq!(fetched.integrity, INTEGRITY_UNVERIFIED);
    assert!(fetched.content.is_empty());
    let failure = fetched.failure("charless-mac-mini").unwrap();
    assert!(failure.contains(FILE_REFUSED_SYMLINK), "{failure}");
    assert!(failure.contains("was not followed"), "{failure}");
}

#[test]
fn the_path_comes_back_base64_and_is_decoded() {
    let fetched_size = b"one".len();
    let payload = format!(
        r#"{{"path":"{}","file_state":"read","detail":"","mode":"700","owner_only":true,"bytes":{},"digest":"{}","content_b64":"{}"}}"#,
        STANDARD.encode("/Users/charles/a b\"c"),
        fetched_size,
        digest_of(b"one"),
        STANDARD.encode("one"),
    );
    let report = parse_fetch(&format!("Welcome to macOS\n{payload}\n")).unwrap();
    assert_eq!(report.path, "/Users/charles/a b\"c");
    assert_eq!(verify(report).integrity, INTEGRITY_VERIFIED);
}

#[test]
fn the_script_carries_the_path_only_base64_and_never_literally() {
    let script = remote_fetch_script("$HOME/.stado/bin/weles-release-cutover");
    assert!(!script.contains("weles-release-cutover"), "{script}");
    assert!(script.contains(&STANDARD.encode("$HOME/.stado/bin/weles-release-cutover")));
    assert!(script.contains(&MAX_FETCH_BYTES.to_string()));
}
