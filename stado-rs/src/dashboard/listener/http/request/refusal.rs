//! A request refused before it reaches a route, carrying the HTTP status and
//! reason phrase that answer it. They travel with the sentence inside the
//! read's `std::io::Error`, so a listener answers with them instead of
//! guessing the status from the sentence's words.

use reqwest::StatusCode;

/// Why a request was refused, the status that says so, and the status's
/// reason phrase as its RFC names it (reqwest still spells Content Too Large
/// by its older name).
#[derive(Debug)]
pub(crate) struct Refusal {
    pub(crate) status: StatusCode,
    pub(crate) reason: &'static str,
    sentence: String,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.sentence)
    }
}

impl std::error::Error for Refusal {}

impl Refusal {
    /// The refusal a request read error carries; a read that failed for any
    /// other reason (the socket itself) carries none.
    pub(crate) fn of(error: &std::io::Error) -> Option<&Refusal> {
        error.get_ref()?.downcast_ref()
    }
}

fn refuse(status: StatusCode, reason: &'static str, sentence: String) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        Refusal {
            status,
            reason,
            sentence,
        },
    )
}

/// A request that is not HTTP/1.1 as RFC 9112 frames it
/// (https://www.rfc-editor.org/rfc/rfc9110#name-400-bad-request).
pub(super) fn malformed(sentence: impl Into<String>) -> std::io::Error {
    refuse(StatusCode::BAD_REQUEST, "Bad Request", sentence.into())
}

/// A write that must name its length and names none
/// (https://www.rfc-editor.org/rfc/rfc9110#name-411-length-required).
pub(super) fn length_required(sentence: impl Into<String>) -> std::io::Error {
    refuse(
        StatusCode::LENGTH_REQUIRED,
        "Length Required",
        sentence.into(),
    )
}

/// A transfer coding this listener does not frame; RFC 9112 asks a server
/// that receives one to answer Not Implemented
/// (https://www.rfc-editor.org/rfc/rfc9112#section-6.1).
pub(super) fn not_implemented(sentence: impl Into<String>) -> std::io::Error {
    refuse(
        StatusCode::NOT_IMPLEMENTED,
        "Not Implemented",
        sentence.into(),
    )
}

/// A body past its route's bound
/// (https://www.rfc-editor.org/rfc/rfc9110#name-413-content-too-large).
pub(super) fn content_too_large(sentence: impl Into<String>) -> std::io::Error {
    refuse(
        StatusCode::PAYLOAD_TOO_LARGE,
        "Content Too Large",
        sentence.into(),
    )
}

/// A head past the head bound
/// (https://www.rfc-editor.org/rfc/rfc6585#section-5).
pub(super) fn head_too_large(sentence: impl Into<String>) -> std::io::Error {
    refuse(
        StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE,
        "Request Header Fields Too Large",
        sentence.into(),
    )
}
