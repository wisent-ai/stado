//! `stado product crx3`: a browser extension directory as a signed CRX3 file
//! and the Omaha update manifest that points browsers at it. Skarbiec's
//! release built its autofill extension with a Python script of its own,
//! `tools/package-crx3.py`; this is that step.
//!
//! CRX3 layout: `Cr24`, format version, header length, then a `CrxFileHeader`
//! protobuf carrying the RSA public key and a PKCS#1 v1.5 SHA-256 signature
//! over `CRX3 SignedData\0` + the signed header data's length + that data +
//! the zip. The extension id is the first sixteen bytes of sha256(public key)
//! spelled with the letters a–p, so re-signing with the pinned key keeps the
//! id stable; a key that yields another id is refused before anything is
//! written, because the native messaging manifest pins that id.

mod zip;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

const CRX_MAGIC: &[u8] = b"Cr24";
const CRX_FORMAT_VERSION: u32 = 3;
const SIGNED_DATA_PREFIX: &[u8] = b"CRX3 SignedData\0";
const CRX_ID_BYTES: usize = 16;
const ID_ALPHABET: &[u8] = b"abcdefghijklmnop";
const NIBBLE_BITS: u32 = 4;
const NIBBLE_MASK: u8 = 0x0f;
const LENGTH_BYTES: usize = 4;

/// `CrxFileHeader` / `AsymmetricKeyProof` / `SignedData` field numbers.
const FIELD_PUBLIC_KEY_OR_CRX_ID: u64 = 1;
const FIELD_SIGNATURE_OR_RSA_PROOF: u64 = 2;
const FIELD_SIGNED_HEADER_DATA: u64 = 10000;
const WIRE_LENGTH_DELIMITED: u64 = 2;
const TAG_SHIFT: u32 = 3;
const VARINT_PAYLOAD: u64 = 0x7f;
const VARINT_CONTINUE: u8 = 0x80;
const VARINT_SHIFT: u32 = 7;

/// Chrome's version grammar: one to four dot-separated integers, each at most
/// 65535 and written without leading zeros.
const VERSION_MAX_PARTS: usize = 4;

fn varint(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let byte = (value & VARINT_PAYLOAD) as u8;
        value >>= VARINT_SHIFT;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | VARINT_CONTINUE);
    }
}

fn field(tag: u64, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + LENGTH_BYTES * 2);
    varint((tag << TAG_SHIFT) | WIRE_LENGTH_DELIMITED, &mut out);
    varint(payload.len() as u64, &mut out);
    out.extend_from_slice(payload);
    out
}

fn u32_le(value: usize) -> Result<[u8; LENGTH_BYTES]> {
    Ok(u32::try_from(value)
        .context("a CRX3 length exceeds four bytes")?
        .to_le_bytes())
}

fn validate_version(version: &str) -> Result<()> {
    let parts: Vec<&str> = version.split('.').collect();
    let valid = parts.len() <= VERSION_MAX_PARTS
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_digit())
                && part.parse::<u16>().is_ok_and(|n| n.to_string() == *part)
        });
    if !valid {
        bail!("invalid Chrome extension version: {version}");
    }
    Ok(())
}

fn extension_id(crx_id: &[u8]) -> String {
    crx_id
        .iter()
        .flat_map(|byte| [byte >> NIBBLE_BITS, byte & NIBBLE_MASK])
        .map(|nibble| ID_ALPHABET[nibble as usize] as char)
        .collect()
}

fn openssl(arguments: &[&std::ffi::OsStr]) -> Result<Vec<u8>> {
    let output = Command::new("openssl")
        .args(arguments)
        .output()
        .context("cannot run openssl")?;
    if !output.status.success() {
        bail!(
            "openssl {} failed: {}",
            arguments
                .first()
                .map(|a| a.to_string_lossy().into_owned())
                .unwrap_or_default(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

pub struct Request {
    pub extension: PathBuf,
    pub key: PathBuf,
    pub expected_id: String,
    pub codebase: String,
    pub version: String,
    pub crx: PathBuf,
    pub update_manifest: PathBuf,
}

pub fn run(request: &Request) -> Result<i32> {
    validate_version(&request.version)?;
    let public_key = openssl(&[
        "rsa".as_ref(),
        "-in".as_ref(),
        request.key.as_os_str(),
        "-pubout".as_ref(),
        "-outform".as_ref(),
        "DER".as_ref(),
    ])?;
    let crx_id = &Sha256::digest(&public_key)[..CRX_ID_BYTES];
    let app_id = extension_id(crx_id);
    if app_id != request.expected_id {
        bail!(
            "signing key produces extension id {app_id}, expected pinned id {}",
            request.expected_id
        );
    }
    let signed_header_data = field(FIELD_PUBLIC_KEY_OR_CRX_ID, crx_id);
    let zip = zip::extension(&request.extension, &request.version)?;

    // openssl reads the signed bytes from a file: they hold the whole zip, and
    // a pipe that size deadlocks a writer that reads the signature afterwards.
    let signed_path = request.crx.with_extension("signed-data");
    let mut signed = Vec::with_capacity(
        SIGNED_DATA_PREFIX.len() + LENGTH_BYTES + signed_header_data.len() + zip.len(),
    );
    signed.extend_from_slice(SIGNED_DATA_PREFIX);
    signed.extend(u32_le(signed_header_data.len())?);
    signed.extend(&signed_header_data);
    signed.extend(&zip);
    write(&signed_path, &signed)?;
    let signature = openssl(&[
        "dgst".as_ref(),
        "-sha256".as_ref(),
        "-sign".as_ref(),
        request.key.as_os_str(),
        signed_path.as_os_str(),
    ]);
    let _ = fs::remove_file(&signed_path);
    let signature = signature?;

    let mut proof = field(FIELD_PUBLIC_KEY_OR_CRX_ID, &public_key);
    proof.extend(field(FIELD_SIGNATURE_OR_RSA_PROOF, &signature));
    let mut header = field(FIELD_SIGNATURE_OR_RSA_PROOF, &proof);
    header.extend(field(FIELD_SIGNED_HEADER_DATA, &signed_header_data));
    let mut crx = Vec::with_capacity(CRX_MAGIC.len() + LENGTH_BYTES * 2 + header.len() + zip.len());
    crx.extend_from_slice(CRX_MAGIC);
    crx.extend(CRX_FORMAT_VERSION.to_le_bytes());
    crx.extend(u32_le(header.len())?);
    crx.extend(&header);
    crx.extend(&zip);
    write(&request.crx, &crx)?;

    let update = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <gupdate xmlns=\"http://www.google.com/update2/response\" protocol=\"2.0\">\n\
         \x20 <app appid=\"{app_id}\">\n\
         \x20   <updatecheck codebase=\"{}\" version=\"{}\"/>\n\
         \x20 </app>\n\
         </gupdate>\n",
        xml_escape(&request.codebase),
        xml_escape(&request.version)
    );
    write(&request.update_manifest, update.as_bytes())?;
    println!(
        "packed {} -> {} and {} (id {app_id})",
        request.extension.display(),
        request.crx.display(),
        request.update_manifest.display()
    );
    Ok(0)
}
