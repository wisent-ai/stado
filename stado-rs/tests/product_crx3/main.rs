//! `stado product crx3` through the real binary with a real openssl key.
//!
//! A browser drops a CRX3 whose signature, id or zip is wrong without saying
//! why, so this reads the packed file back the way a browser does: the header
//! protobuf yields the public key, the signature and the signed header data;
//! openssl verifies that signature over the exact signed bytes; the zip after
//! the header opens and carries the rewritten version; the update manifest
//! names the id and the codebase. A key whose id is not the pinned one is
//! refused before anything is written. Every path lives under Cargo's target
//! directory for this test binary.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const VERSION: &str = "1.2.3";
const CODEBASE: &str = "https://stado.wisent.com/releases/skarbiec/1.2.3/linux-amd64/a&b.crx";

fn scratch(label: &str) -> PathBuf {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("crx3-{label}"));
    if root.exists() {
        fs::remove_dir_all(&root).expect("clear the scratch root");
    }
    fs::create_dir_all(root.join("extension/icons")).expect("create the extension");
    fs::write(
        root.join("extension/manifest.json"),
        "{\"manifest_version\": 3, \"name\": \"journey\", \"version\": \"0.0.1\"}\n",
    )
    .unwrap();
    fs::write(root.join("extension/background.js"), "chrome.runtime.onInstalled.addListener(() => {});\n").unwrap();
    fs::write(root.join("extension/icons/icon.txt"), "icon\n".repeat(4096)).unwrap();
    root
}

fn run(program: &str, arguments: &[&str]) -> Output {
    Command::new(program)
        .args(arguments)
        .output()
        .unwrap_or_else(|error| panic!("cannot run {program}: {error}"))
}

fn key(root: &Path, name: &str) -> String {
    let path = root.join(name).to_string_lossy().into_owned();
    let made = run("openssl", &["genrsa", "-out", &path, "2048"]);
    assert!(made.status.success(), "{made:?}");
    path
}

/// The id a browser derives: sha256 of the DER public key, first sixteen bytes
/// spelled a–p, computed here with openssl alone.
fn extension_id(root: &Path, key: &str) -> String {
    let public = root.join("public.der").to_string_lossy().into_owned();
    assert!(run("openssl", &["rsa", "-in", key, "-pubout", "-outform", "DER", "-out", &public]).status.success());
    let digest = run("openssl", &["dgst", "-sha256", "-binary", &public]).stdout;
    digest[..16]
        .iter()
        .flat_map(|b| [b >> 4, b & 0x0f])
        .map(|n| (b'a' + n) as char)
        .collect()
}

fn pack(root: &Path, key: &str, expected_id: &str) -> Output {
    let extension = root.join("extension");
    let crx = root.join("out/skarbiec-autofill.crx");
    let update = root.join("out/skarbiec-autofill.xml");
    Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["product", "crx3", "--extension"])
        .arg(&extension)
        .args(["--key", key, "--expected-id", expected_id, "--codebase", CODEBASE, "--version", VERSION, "--crx"])
        .arg(&crx)
        .arg("--update-manifest")
        .arg(&update)
        .output()
        .expect("run stado product crx3")
}

fn varint(bytes: &[u8], at: &mut usize) -> u64 {
    let mut value = 0u64;
    let mut shift = 0;
    loop {
        let byte = bytes[*at];
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
        shift += 7;
    }
}

/// Length-delimited fields of one protobuf message, by field number.
fn fields(bytes: &[u8]) -> Vec<(u64, Vec<u8>)> {
    let mut at = 0;
    let mut out = Vec::new();
    while at < bytes.len() {
        let key = varint(bytes, &mut at);
        assert_eq!(key & 7, 2, "every CRX3 header field is length-delimited");
        let length = varint(bytes, &mut at) as usize;
        out.push((key >> 3, bytes[at..at + length].to_vec()));
        at += length;
    }
    out
}

fn one(fields: &[(u64, Vec<u8>)], number: u64) -> Vec<u8> {
    let found: Vec<_> = fields.iter().filter(|(n, _)| *n == number).collect();
    assert_eq!(found.len(), 1, "field {number}");
    found[0].1.clone()
}

#[test]
fn a_packed_extension_verifies_and_carries_its_version_and_update_manifest() {
    let root = scratch("pack");
    let key = key(&root, "key.pem");
    let id = extension_id(&root, &key);
    let packed = pack(&root, &key, &id);
    assert!(packed.status.success(), "{packed:?}");

    let crx = fs::read(root.join("out/skarbiec-autofill.crx")).unwrap();
    assert_eq!(&crx[..4], b"Cr24");
    assert_eq!(u32::from_le_bytes(crx[4..8].try_into().unwrap()), 3);
    let header_length = u32::from_le_bytes(crx[8..12].try_into().unwrap()) as usize;
    let header = fields(&crx[12..12 + header_length]);
    let zip = &crx[12 + header_length..];
    let proof = fields(&one(&header, 2));
    let public_key = one(&proof, 1);
    let signature = one(&proof, 2);
    let signed_header_data = one(&header, 10000);
    assert_eq!(fs::read(root.join("public.der")).unwrap(), public_key);

    let mut signed = b"CRX3 SignedData\0".to_vec();
    signed.extend((signed_header_data.len() as u32).to_le_bytes());
    signed.extend(&signed_header_data);
    signed.extend(zip);
    fs::write(root.join("signed"), &signed).unwrap();
    fs::write(root.join("signature"), &signature).unwrap();
    let verified = run(
        "openssl",
        &[
            "dgst", "-sha256", "-keyform", "DER", "-verify",
            &root.join("public.der").to_string_lossy(),
            "-signature", &root.join("signature").to_string_lossy(),
            &root.join("signed").to_string_lossy(),
        ],
    );
    assert!(verified.status.success(), "{verified:?}");

    fs::write(root.join("extension.zip"), zip).unwrap();
    let manifest = run("unzip", &["-p", &root.join("extension.zip").to_string_lossy(), "manifest.json"]);
    assert!(manifest.status.success(), "{manifest:?}");
    let manifest: serde_json::Value = serde_json::from_slice(&manifest.stdout).unwrap();
    assert_eq!(manifest["version"], VERSION);
    assert_eq!(manifest["name"], "journey");
    let listing = run("unzip", &["-Z1", &root.join("extension.zip").to_string_lossy()]);
    assert_eq!(
        String::from_utf8_lossy(&listing.stdout).lines().collect::<Vec<_>>(),
        ["background.js", "icons/icon.txt", "manifest.json"]
    );
    let tested = run("unzip", &["-t", &root.join("extension.zip").to_string_lossy()]);
    assert!(tested.status.success(), "{tested:?}");

    let update = fs::read_to_string(root.join("out/skarbiec-autofill.xml")).unwrap();
    assert!(update.contains(&format!("<app appid=\"{id}\">")), "{update}");
    assert!(update.contains("codebase=\"https://stado.wisent.com/releases/skarbiec/1.2.3/linux-amd64/a&amp;b.crx\""), "{update}");
    assert!(update.contains("version=\"1.2.3\""), "{update}");
    assert!(!root.join("out/skarbiec-autofill.signed-data").exists(), "the signed-data scratch file stayed");

    let again = pack(&root, &key, &id);
    assert!(again.status.success(), "{again:?}");
    let repacked = fs::read(root.join("out/skarbiec-autofill.crx")).unwrap();
    assert_eq!(&repacked[12 + header_length..], zip, "one tree packs to one zip");
}

#[test]
fn a_key_with_another_id_or_a_bad_version_is_refused_and_nothing_is_written() {
    let root = scratch("refuse");
    let key = key(&root, "key.pem");
    let refused = pack(&root, &key, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    assert!(!refused.status.success(), "{refused:?}");
    assert!(String::from_utf8_lossy(&refused.stderr).contains("expected pinned id aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"), "{refused:?}");
    assert!(!root.join("out").exists(), "a refused pack wrote output");

    let id = extension_id(&root, &key);
    let bad_version = Command::new(env!("CARGO_BIN_EXE_stado"))
        .args(["product", "crx3", "--extension"])
        .arg(root.join("extension"))
        .args(["--key", &key, "--expected-id", &id, "--codebase", CODEBASE, "--version", "1.02", "--crx"])
        .arg(root.join("out/x.crx"))
        .arg("--update-manifest")
        .arg(root.join("out/x.xml"))
        .output()
        .unwrap();
    assert!(!bad_version.status.success(), "{bad_version:?}");
    assert!(String::from_utf8_lossy(&bad_version.stderr).contains("invalid Chrome extension version: 1.02"), "{bad_version:?}");
    assert!(!root.join("out").exists(), "a refused pack wrote output");
}
