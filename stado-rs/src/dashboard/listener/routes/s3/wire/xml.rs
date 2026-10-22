//! The S3 API's wire vocabulary on this listener: the XML document every
//! answer and every refusal is, the error shape S3 clients parse, and the
//! few request documents the endpoint reads (`Delete`,
//! `CompleteMultipartUpload`).

use crate::dashboard::listener::{http_status, Response};

pub(crate) const XML: &str = "application/xml";
const DECLARATION: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";

/// `text` with the five XML-special characters escaped.
pub(crate) fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

/// `text` with XML entity and character references resolved, or why it
/// cannot be.
pub(crate) fn unescape(text: &str) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let end = after
            .find(';')
            .ok_or_else(|| format!("the reference at {:?} is not closed by ';'", &rest[at..]))?;
        let name = &after[..end];
        let resolved = match name {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let code = if let Some(hex) = name.strip_prefix("#x") {
                    u32::from_str_radix(hex, 16)
                } else if let Some(decimal) = name.strip_prefix('#') {
                    decimal.parse::<u32>()
                } else {
                    return Err(format!("&{name}; is not an XML reference"));
                }
                .map_err(|error| format!("&{name}; is not a character reference: {error}"))?;
                char::from_u32(code)
                    .ok_or_else(|| format!("&{name}; names no Unicode character"))?
            }
        };
        out.push(resolved);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// The text of every `<tag>…</tag>` element of `document`, in order. The
/// request documents the endpoint reads nest no element inside one of the
/// same name, so the first closing tag after an opening one ends it.
pub(crate) fn elements<'a>(document: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut found = Vec::new();
    let mut rest = document;
    while let Some(start) = rest.find(&open) {
        let inner = &rest[start + open.len()..];
        let Some(end) = inner.find(&close) else {
            break;
        };
        found.push(&inner[..end]);
        rest = &inner[end + close.len()..];
    }
    found
}

/// The text of the first `<tag>` element, unescaped.
pub(crate) fn element(document: &str, tag: &str) -> Result<Option<String>, String> {
    match elements(document, tag).first() {
        Some(text) => unescape(text).map(Some),
        None => Ok(None),
    }
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
    /// as well.
    pub(crate) fn response(&self, resource: &str, head: bool) -> Response {
        let reason = match self.status.canonical_reason() {
            Some(reason) => reason,
            None => "",
        };
        let headers = [
            ("x-amz-error-code", self.code.to_string()),
            ("x-amz-error-message", header_safe(&self.message)),
        ];
        if head || self.status == reqwest::StatusCode::NOT_MODIFIED {
            return Response::headers_only(http_status(self.status), reason, XML, 0, &headers);
        }
        let body = format!(
            "{DECLARATION}<Error><Code>{}</Code><Message>{}</Message><Resource>{}</Resource></Error>",
            self.code,
            escape(&self.message),
            escape(resource)
        );
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
