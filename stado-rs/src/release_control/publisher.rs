//! The publisher boundary: read the registry's control document, canonicalise
//! a manifest, digest the bytes it attests, sign and verify it, and name the
//! immutable prefix a coordinate is published under.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::release_control::{identifier, ReleaseControl, ReleaseManifest, RELEASE_CONTROL_KEY};

pub fn control(document: &Value) -> Result<Option<ReleaseControl>, String> {
    document
        .get(RELEASE_CONTROL_KEY)
        .map(|value| {
            serde_json::from_value(value.clone())
                .map_err(|error| format!("registry.{RELEASE_CONTROL_KEY}: {error}"))
        })
        .transpose()
}

pub fn canonical_manifest(manifest: &ReleaseManifest) -> Result<Vec<u8>, String> {
    serde_json::to_vec(manifest).map_err(|error| format!("cannot encode release manifest: {error}"))
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// A release artifact whose digest could not be taken: the file could not be
/// opened, inspected or read, or it is not a non-empty regular file.
#[derive(Debug, thiserror::Error)]
pub enum ArtifactDigestError {
    #[error("cannot {step} release artifact {path}: {error}")]
    Io {
        step: &'static str,
        path: String,
        error: std::io::Error,
    },
    #[error("release artifact {0} must be a non-empty regular file")]
    NotARegularFile(String),
}

pub fn sha256_file(path: &Path) -> Result<(u64, String), ArtifactDigestError> {
    let failed = |step: &'static str| {
        move |error: std::io::Error| ArtifactDigestError::Io {
            step,
            path: path.display().to_string(),
            error,
        }
    };
    let mut file = File::open(path).map_err(failed("open"))?;
    let metadata = file.metadata().map_err(failed("stat"))?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(ArtifactDigestError::NotARegularFile(
            path.display().to_string(),
        ));
    }
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(failed("read"))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok((metadata.len(), hex::encode(digest.finalize())))
}

pub fn generate_signing_key() -> Result<(Vec<u8>, Vec<u8>), String> {
    let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
        .map_err(|_| "could not generate Ed25519 release signing key".to_string())?;
    let key = Ed25519KeyPair::from_pkcs8(document.as_ref())
        .map_err(|_| "generated Ed25519 release signing key is invalid".to_string())?;
    Ok((
        document.as_ref().to_vec(),
        key.public_key().as_ref().to_vec(),
    ))
}

pub fn signing_public_key(private_pkcs8: &[u8]) -> Result<Vec<u8>, String> {
    let key = Ed25519KeyPair::from_pkcs8(private_pkcs8)
        .map_err(|_| "release signing key is not Ed25519 PKCS#8".to_string())?;
    Ok(key.public_key().as_ref().to_vec())
}

pub fn sign_manifest(private_pkcs8: &[u8], manifest: &ReleaseManifest) -> Result<String, String> {
    let key = Ed25519KeyPair::from_pkcs8(private_pkcs8)
        .map_err(|_| "release signing key is not Ed25519 PKCS#8".to_string())?;
    Ok(BASE64.encode(key.sign(&canonical_manifest(manifest)?).as_ref()))
}

pub fn verify_manifest(
    public_key_b64: &str,
    manifest: &ReleaseManifest,
    signature_b64: &str,
) -> Result<(), String> {
    let public_key = BASE64
        .decode(public_key_b64)
        .map_err(|_| "trusted release public key is not base64".to_string())?;
    let signature = BASE64
        .decode(signature_b64.trim())
        .map_err(|_| "release signature is not base64".to_string())?;
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(&canonical_manifest(manifest)?, &signature)
        .map_err(|_| "release manifest signature verification failed".to_string())
}

pub fn release_base(product: &str, version: &str, platform: &str) -> Result<String, String> {
    if !identifier(product) || !identifier(version) || !identifier(platform) {
        return Err(
            "release product, version, and platform must be canonical coordinates".to_string(),
        );
    }
    Ok(format!("stado://releases/{product}/{version}/{platform}"))
}

pub fn release_version_base(product: &str, version: &str) -> Result<String, String> {
    if !identifier(product) || !identifier(version) {
        return Err("release product and version must be canonical coordinates".to_string());
    }
    Ok(format!("stado://releases/{product}/{version}"))
}
