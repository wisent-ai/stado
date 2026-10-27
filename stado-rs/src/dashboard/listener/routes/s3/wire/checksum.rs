//! The integrity values an S3 client may send with a body, recomputed here
//! so a body the transport damaged is refused rather than stored: the
//! `x-amz-checksum-*` family (CRC32, CRC32C, CRC64NVME, SHA-1, SHA-256), as a
//! header or as a trailer of an aws-chunked body, and `Content-MD5`. Each is
//! the base64 of the digest's big-endian bytes.
//!
//! Each algorithm is computed by the crate that defines it, never by hand:
//! the CRCs by the `crc` crate's catalogue under the names the CRC catalogue
//! gives the S3 algorithms (CRC32 is CRC-32/ISO-HDLC, CRC32C is
//! CRC-32/ISCSI, the Castagnoli polynomial, CRC64NVME is CRC-64/NVME), SHA-1
//! and SHA-256 by `ring`, and MD5 (RFC 1321) by the `md5` crate.

use base64::Engine;
use crc::{Crc, CRC_32_ISCSI, CRC_32_ISO_HDLC, CRC_64_NVME};

static CRC32: Crc<u32> = Crc::<u32>::new(&CRC_32_ISO_HDLC);
static CRC32C: Crc<u32> = Crc::<u32>::new(&CRC_32_ISCSI);
static CRC64NVME: Crc<u64> = Crc::<u64>::new(&CRC_64_NVME);

/// The base64 digest `algorithm` computes over `data`, or `None` for an
/// algorithm this endpoint does not compute. `algorithm` is the suffix of an
/// `x-amz-checksum-*` name, or `content-md5`.
pub(crate) fn digest(algorithm: &str, data: &[u8]) -> Option<String> {
    let bytes: Vec<u8> = match algorithm {
        "crc32" => CRC32.checksum(data).to_be_bytes().to_vec(),
        "crc32c" => CRC32C.checksum(data).to_be_bytes().to_vec(),
        "crc64nvme" => CRC64NVME.checksum(data).to_be_bytes().to_vec(),
        "sha1" => ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, data)
            .as_ref()
            .to_vec(),
        "sha256" => ring::digest::digest(&ring::digest::SHA256, data)
            .as_ref()
            .to_vec(),
        "content-md5" => md5::compute(data).0.to_vec(),
        _ => return None,
    };
    Some(base64::engine::general_purpose::STANDARD.encode(bytes))
}
