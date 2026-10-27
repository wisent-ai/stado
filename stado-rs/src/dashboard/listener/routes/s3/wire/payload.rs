//! The bytes one signed write carries, recovered and proved before they are
//! stored: the body checked against the SHA-256 its signature covers, or an
//! aws-chunked body unframed with each chunk signature and the trailer
//! signature verified, and then every checksum the client stated, in a
//! header or a trailer, recomputed (`checksum`).
//!
//! The framing is AWS's,
//! <https://docs.aws.amazon.com/AmazonS3/latest/API/sigv4-streaming.html>.

use base64::Engine;
use reqwest::StatusCode;

use crate::dashboard::listener::{constant_time_eq, Request};
use crate::remote::sigv4::{hex_sha256, signature};

use super::super::auth::{Payload, Signer};
use super::checksum::digest;
use super::xml::S3Error;

const LINE_END: &[u8] = b"\r\n";
// AWS declares hexadecimal chunk sizes:
// https://docs.aws.amazon.com/AmazonS3/latest/userguide/checking-object-integrity-upload.html#trailing-checksums-object-body-chunks-format
const CHUNK_RADIX: u32 = 16;

type TrailerHeaders = Vec<(String, String)>;

fn malformed(message: impl Into<String>) -> S3Error {
    S3Error::new(StatusCode::BAD_REQUEST, "IncompleteBody", message)
}

fn forged(message: impl Into<String>) -> S3Error {
    S3Error::new(StatusCode::FORBIDDEN, "SignatureDoesNotMatch", message)
}

/// One `\r\n`-ended line of `body` from `at`, and where the next one starts.
fn line(body: &[u8], at: usize) -> Result<(&str, usize), S3Error> {
    let rest = &body[at.min(body.len())..];
    let end = rest
        .windows(LINE_END.len())
        .position(|window| window == LINE_END)
        .ok_or_else(|| {
            malformed(format!(
                "the aws-chunked body ends at byte {at} inside a line"
            ))
        })?;
    let text = std::str::from_utf8(&rest[..end]).map_err(|error| {
        malformed(format!(
            "the aws-chunked line at byte {at} is not UTF-8: {error}"
        ))
    })?;
    Ok((text, at + end + LINE_END.len()))
}

/// Unframe an aws-chunked body, verifying chunk and trailer signatures when
/// they are signed, and answer the data and the trailing headers.
fn dechunk(
    body: &[u8],
    signer: &Signer,
    signed: bool,
    trailer: bool,
) -> Result<(Vec<u8>, TrailerHeaders), S3Error> {
    let scope = signer.scope.render();
    let mut data = Vec::with_capacity(body.len());
    let mut previous = signer.seed.clone();
    // Slice offsets are zero-based:
    // https://doc.rust-lang.org/reference/expressions/array-expr.html#r-expr.array.index.zero-index
    let mut at = 0;
    loop {
        let (header, next) = line(body, at)?;
        let (size, extension) = match header.split_once(';') {
            Some((size, extension)) => (size, extension),
            None => (header, ""),
        };
        let size = usize::from_str_radix(size.trim(), CHUNK_RADIX).map_err(|error| {
            malformed(format!(
                "the chunk size {size:?} at byte {at} is not hexadecimal: {error}"
            ))
        })?;
        let end = next
            .checked_add(size)
            .filter(|end| *end <= body.len())
            .ok_or_else(|| {
                malformed(format!(
                    "a chunk of {size} bytes at byte {next} runs past the {}-byte body",
                    body.len()
                ))
            })?;
        let chunk = &body[next..end];
        if signed {
            let provided = extension.strip_prefix("chunk-signature=").ok_or_else(|| {
                forged(format!("the chunk at byte {at} carries no chunk-signature"))
            })?;
            let expected = signature(
                &signer.key,
                &format!(
                    "AWS4-HMAC-SHA256-PAYLOAD\n{}\n{scope}\n{previous}\n{}\n{}",
                    signer.amz_date,
                    hex_sha256(b""),
                    hex_sha256(chunk)
                ),
            );
            if !constant_time_eq(expected.as_bytes(), provided.as_bytes()) {
                return Err(forged(format!(
                    "the signature of the chunk at byte {at} does not match its bytes"
                )));
            }
            previous = provided.to_string();
        }
        data.extend_from_slice(chunk);
        if chunk.is_empty() {
            at = next;
            break;
        }
        at = end;
        if body.get(at..at + LINE_END.len()) != Some(LINE_END) {
            return Err(malformed(format!(
                "the chunk ending at byte {at} is not followed by CRLF"
            )));
        }
        at += LINE_END.len();
    }
    let mut trailers = Vec::new();
    if !trailer {
        if body.get(at..) != Some(LINE_END) {
            return Err(malformed(format!(
                "the aws-chunked body continues past its final chunk at byte {at}"
            )));
        }
        return Ok((data, trailers));
    }
    let mut trailer_signature = None;
    loop {
        let (text, next) = line(body, at)?;
        at = next;
        if text.is_empty() {
            break;
        }
        let (name, value) = text
            .split_once(':')
            .ok_or_else(|| malformed(format!("the trailing header {text:?} has no ':'")))?;
        let name = name.trim().to_ascii_lowercase();
        if name == "x-amz-trailer-signature" {
            trailer_signature = Some(value.trim().to_string());
        } else {
            trailers.push((name, value.trim().to_string()));
        }
    }
    if at != body.len() {
        return Err(malformed(format!(
            "the aws-chunked body continues past its trailer at byte {at}"
        )));
    }
    if signed {
        let provided = trailer_signature
            .ok_or_else(|| forged("the signed trailer carries no x-amz-trailer-signature"))?;
        let canonical: String = trailers
            .iter()
            .map(|(name, value)| format!("{name}:{value}\n"))
            .collect();
        let expected = signature(
            &signer.key,
            &format!(
                "AWS4-HMAC-SHA256-TRAILER\n{}\n{scope}\n{previous}\n{}",
                signer.amz_date,
                hex_sha256(canonical.as_bytes())
            ),
        );
        if !constant_time_eq(expected.as_bytes(), provided.as_bytes()) {
            return Err(forged(
                "the trailer signature does not match the trailing headers",
            ));
        }
    }
    Ok((data, trailers))
}

/// Every digest the client stated for the body, from its headers and its
/// trailers, as (algorithm, base64 value). An `x-amz-checksum-` field whose
/// value is no base64 at all carries no digest (the S3 API also sends a
/// checksum's type, mode and algorithm name under that prefix) and is not a
/// statement about the body.
fn stated_digests(request: &Request, trailers: &[(String, String)]) -> Vec<(String, String)> {
    let mut stated = Vec::new();
    for (name, value) in request.headers.iter().chain(trailers.iter()) {
        if let Some(algorithm) = name.strip_prefix("x-amz-checksum-") {
            let decodes = base64::engine::general_purpose::STANDARD
                .decode(value.trim())
                .is_ok_and(|bytes| !bytes.is_empty());
            if decodes {
                stated.push((algorithm.to_string(), value.clone()));
            }
        } else if name == "content-md5" {
            stated.push((name.clone(), value.clone()));
        }
    }
    stated
}

/// The body of one signed write, unframed and proved.
pub(crate) fn body(request: &Request, signer: &Signer) -> Result<Vec<u8>, S3Error> {
    let (data, trailers) = match &signer.payload {
        Payload::Sha256(declared) => {
            let computed = hex_sha256(&request.body);
            if &computed != declared {
                return Err(S3Error::new(
                    StatusCode::BAD_REQUEST,
                    "XAmzContentSHA256Mismatch",
                    format!(
                        "the body's SHA-256 is {computed}, the signature covers {declared}: the \
                         body changed after it was signed"
                    ),
                ));
            }
            (request.body.clone(), Vec::new())
        }
        Payload::Unsigned => (request.body.clone(), Vec::new()),
        Payload::Streaming { signed, trailer } => {
            dechunk(&request.body, signer, *signed, *trailer)?
        }
    };
    if let Some(declared) = request.header("x-amz-decoded-content-length") {
        let length = declared.parse::<usize>().map_err(|error| {
            malformed(format!(
                "x-amz-decoded-content-length {declared:?} is not a byte count: {error}"
            ))
        })?;
        if length != data.len() {
            return Err(malformed(format!(
                "x-amz-decoded-content-length is {length} and the body unframes to {} bytes",
                data.len()
            )));
        }
    }
    if let Some(announced) = request.header("x-amz-trailer") {
        for name in announced
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            if !trailers
                .iter()
                .any(|(trailer, _)| trailer.eq_ignore_ascii_case(name))
            {
                return Err(malformed(format!(
                    "x-amz-trailer announces {name} and the body carries no such trailer"
                )));
            }
        }
    }
    for (algorithm, stated) in stated_digests(request, &trailers) {
        let Some(computed) = digest(&algorithm, &data) else {
            return Err(S3Error::new(
                StatusCode::BAD_REQUEST,
                "InvalidRequest",
                format!(
                    "x-amz-checksum-{algorithm} names an algorithm this endpoint does not \
                     compute; state crc32, crc32c, crc64nvme, sha1, sha256 or Content-MD5"
                ),
            ));
        };
        if computed != stated.trim() {
            return Err(S3Error::new(
                StatusCode::BAD_REQUEST,
                "BadDigest",
                format!(
                    "the {algorithm} you stated is {}; the received body computes {computed}",
                    stated.trim()
                ),
            ));
        }
    }
    Ok(data)
}
