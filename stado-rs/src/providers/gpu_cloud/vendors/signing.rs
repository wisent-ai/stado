//! Signatures the vendors that do not take a bearer key require: RSA-SHA256
//! with a PEM private key (Oracle request signing, Nebius service-account
//! JWTs) and HMAC-SHA256 (Crusoe). The key material comes from Skarbiec and
//! never leaves this module in an error.

use base64::Engine as _;

use crate::capabilities::GpuCloudVendor;
use crate::providers::gpu_cloud::api::GpuCloudError;

/// The DER bytes inside a PEM block.
fn pem_der(vendor: GpuCloudVendor, pem: &str) -> Result<Vec<u8>, GpuCloudError> {
    let body: String = pem
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("-----"))
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(body)
        .map_err(|error| {
            GpuCloudError::Credential(format!(
                "{}: the private key in Skarbiec role {} is not PEM: {error}",
                vendor.display_name(),
                vendor.credential_role()
            ))
        })
}

/// RSASSA-PKCS1-v1_5 SHA-256 over `message` with a PKCS#8 or PKCS#1 PEM key.
pub fn rsa_sha256(
    vendor: GpuCloudVendor,
    pem: &str,
    message: &[u8],
) -> Result<Vec<u8>, GpuCloudError> {
    let der = pem_der(vendor, pem)?;
    let key = ring::signature::RsaKeyPair::from_pkcs8(&der)
        .or_else(|_| ring::signature::RsaKeyPair::from_der(&der))
        .map_err(|error| {
            GpuCloudError::Credential(format!(
                "{}: the private key in Skarbiec role {} is not an RSA key ring accepts: {error}",
                vendor.display_name(),
                vendor.credential_role()
            ))
        })?;
    let mut signature: Vec<u8> = Vec::new();
    signature.resize(key.public().modulus_len(), Default::default());
    key.sign(
        &ring::signature::RSA_PKCS1_SHA256,
        &ring::rand::SystemRandom::new(),
        message,
        &mut signature,
    )
    .map_err(|error| {
        GpuCloudError::Credential(format!(
            "{}: RSA signing failed: {error}",
            vendor.display_name()
        ))
    })?;
    Ok(signature)
}

/// HMAC-SHA256 of `message` under `secret`.
pub fn hmac_sha256(secret: &[u8], message: &[u8]) -> Vec<u8> {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret);
    ring::hmac::sign(&key, message).as_ref().to_vec()
}

/// Standard base64 with padding.
pub fn base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// URL-safe base64 without padding (JWT segments, Crusoe signatures).
pub fn base64_url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
