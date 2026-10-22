//! The integrity values an S3 client may send with a body, recomputed here
//! so a body the transport damaged is refused rather than stored: the
//! `x-amz-checksum-*` family (CRC32, CRC32C, CRC64NVME, SHA-1, SHA-256), as a
//! header or as a trailer of an aws-chunked body, and `Content-MD5`. Each is
//! the base64 of the digest's big-endian bytes.
//!
//! The polynomials are the ones each algorithm is defined by: CRC32 is
//! ISO-HDLC (reflected 0xEDB88320), CRC32C is Castagnoli (reflected
//! 0x82F63B78), CRC64NVME is the NVMe CRC64 (reflected 0x9A6C9329AC4BC9B5),
//! all starting from all ones and inverted at the end; MD5 is RFC 1321.

use base64::Engine;

const fn table32(polynomial: u32) -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 != 0 {
                (value >> 1) ^ polynomial
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

const fn table64(polynomial: u64) -> [u64; 256] {
    let mut table = [0u64; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u64;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 != 0 {
                (value >> 1) ^ polynomial
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

static CRC32: [u32; 256] = table32(0xEDB8_8320);
static CRC32C: [u32; 256] = table32(0x82F6_3B78);
static CRC64NVME: [u64; 256] = table64(0x9A6C_9329_AC4B_C9B5);

fn crc32_with(table: &[u32; 256], data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in data {
        crc = table[((crc ^ u32::from(byte)) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

fn crc64_with(table: &[u64; 256], data: &[u8]) -> u64 {
    let mut crc = !0u64;
    for &byte in data {
        crc = table[((crc ^ u64::from(byte)) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

/// MD5 (RFC 1321): S3's `Content-MD5` and nothing else in Stado.
fn md5(data: &[u8]) -> [u8; 16] {
    const SHIFTS: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let mut constants = [0u32; 64];
    for (index, constant) in constants.iter_mut().enumerate() {
        *constant = ((index as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32;
    }
    let mut state: [u32; 4] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476];
    let mut message = data.to_vec();
    let bit_length = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_length.to_le_bytes());
    for block in message.chunks_exact(64) {
        let mut words = [0u32; 16];
        for (index, word) in words.iter_mut().enumerate() {
            let at = index * 4;
            *word = u32::from_le_bytes([block[at], block[at + 1], block[at + 2], block[at + 3]]);
        }
        let [mut a, mut b, mut c, mut d] = state;
        for round in 0..64 {
            let (mixed, word) = match round / 16 {
                0 => ((b & c) | (!b & d), round),
                1 => ((d & b) | (!d & c), (5 * round + 1) % 16),
                2 => (b ^ c ^ d, (3 * round + 5) % 16),
                _ => (c ^ (b | !d), (7 * round) % 16),
            };
            let rotated = a
                .wrapping_add(mixed)
                .wrapping_add(constants[round])
                .wrapping_add(words[word])
                .rotate_left(SHIFTS[round]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(rotated);
        }
        state[0] = state[0].wrapping_add(a);
        state[1] = state[1].wrapping_add(b);
        state[2] = state[2].wrapping_add(c);
        state[3] = state[3].wrapping_add(d);
    }
    let mut digest = [0u8; 16];
    for (index, word) in state.iter().enumerate() {
        digest[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    digest
}

/// The base64 digest `algorithm` computes over `data`, or `None` for an
/// algorithm this endpoint does not compute. `algorithm` is the suffix of an
/// `x-amz-checksum-*` name, or `content-md5`.
pub(crate) fn digest(algorithm: &str, data: &[u8]) -> Option<String> {
    let bytes: Vec<u8> = match algorithm {
        "crc32" => crc32_with(&CRC32, data).to_be_bytes().to_vec(),
        "crc32c" => crc32_with(&CRC32C, data).to_be_bytes().to_vec(),
        "crc64nvme" => crc64_with(&CRC64NVME, data).to_be_bytes().to_vec(),
        "sha1" => ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, data)
            .as_ref()
            .to_vec(),
        "sha256" => ring::digest::digest(&ring::digest::SHA256, data)
            .as_ref()
            .to_vec(),
        "content-md5" => md5(data).to_vec(),
        _ => return None,
    };
    Some(base64::engine::general_purpose::STANDARD.encode(bytes))
}
