//! The S3 API's wire vocabulary on this listener: the XML document every
//! answer and every refusal is, the error shape S3 clients parse, and the
//! few request documents the endpoint reads (`Delete`,
//! `CompleteMultipartUpload`), parsed by quick-xml into their typed shapes,
//! so attributes, namespaces, whitespace, CDATA and character references
//! are read as XML defines them.

use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::dashboard::listener::{http_status, Response};

pub(crate) const XML: &str = "application/xml";
const DECLARATION: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";

/// `text` with the five XML-special characters escaped.
pub(crate) fn escape(text: &str) -> String {
    quick_xml::escape::escape(text).into_owned()
}

/// The request document `text` names `what`, read into `T`, or why it is
/// not one.
pub(crate) fn read<T: DeserializeOwned>(text: &str, what: &str) -> Result<T, String> {
    quick_xml::de::from_str(text)
        .map_err(|error| format!("the {what} document is not one: {error}"))
}

/// DeleteObjects' request: whether only failures are answered, and each
/// object to delete.
#[derive(Deserialize)]
pub(crate) struct DeleteDocument {
    #[serde(rename = "Quiet", default)]
    pub(crate) quiet: bool,
    #[serde(rename = "Object", default)]
    pub(crate) objects: Vec<DeleteEntry>,
}

/// One object of a `Delete` document; a missing key is refused by the
/// reader with the code S3 clients branch on.
#[derive(Deserialize)]
pub(crate) struct DeleteEntry {
    #[serde(rename = "Key")]
    pub(crate) key: Option<String>,
}

/// CompleteMultipartUpload's request: the parts, in the order stated.
#[derive(Deserialize)]
pub(crate) struct CompleteDocument {
    #[serde(rename = "Part", default)]
    pub(crate) parts: Vec<CompletedPart>,
}

/// One stated part; a missing number or ETag is refused by the reader as
/// `InvalidPart`, the code S3 answers.
#[derive(Deserialize)]
pub(crate) struct CompletedPart {
    #[serde(rename = "PartNumber")]
    pub(crate) number: Option<String>,
    #[serde(rename = "ETag")]
    pub(crate) etag: Option<String>,
}

/// One refusal in the S3 error vocabulary: the status, the code an S3 client
/// branches on, and a sentence naming what failed, what was observed and
/// what would fix it.
#[derive(Debug)]
pub(crate) struct S3Error {
    pub(crate) status: reqwest::StatusCode,
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl S3Error {
    pub(crate) fn new(
        status: reqwest::StatusCode,
        code: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    /// The error document for `resource`. A `HEAD` answer and a `304` carry
    /// no body (RFC 9110 §15.4.5), so the code travels in `x-amz-error-code`
    /// as well, and their Content-Length is the document's, as a `GET` would
    /// have carried it (RFC 9110 §8.6).
    pub(crate) fn response(&self, resource: &str, head: bool) -> Response {
        let reason = match self.status.canonical_reason() {
            Some(reason) => reason,
            None => "",
        };
        let headers = [
            ("x-amz-error-code", self.code.to_string()),
            ("x-amz-error-message", header_safe(&self.message)),
        ];
        let body = format!(
            "{DECLARATION}<Error><Code>{}</Code><Message>{}</Message><Resource>{}</Resource></Error>",
            self.code,
            escape(&self.message),
            escape(resource)
        );
        if head || self.status == reqwest::StatusCode::NOT_MODIFIED {
            return Response::headers_only(
                http_status(self.status),
                reason,
                XML,
                body.len(),
                &headers,
            );
        }
        Response::new_with_headers(
            http_status(self.status),
            reason,
            XML,
            body.as_bytes(),
            &headers,
        )
    }
}

/// `text` as one header value: every control character, which would end
/// the header or the head, replaced by a space.
fn header_safe(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

/// A successful XML answer.
pub(crate) fn document(status: reqwest::StatusCode, body: &str) -> Response {
    let reason = match status.canonical_reason() {
        Some(reason) => reason,
        None => "",
    };
    Response::new(
        http_status(status),
        reason,
        XML,
        format!("{DECLARATION}{body}").as_bytes(),
    )
}
