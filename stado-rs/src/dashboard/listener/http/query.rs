//! Query-string parsing: `&`-separated `key=value` fields, percent-decoded
//! with `+` as space (the `application/x-www-form-urlencoded` form, WHATWG
//! URL §5.1). A field the decoder cannot read is refused by name, never
//! answered as though the client had sent other bytes.

use serde_json::json;

use super::response::{http_status, send_json, Response};

/// Every field of `query` decoded, in order. A field without `=` names a key
/// with an empty value (WHATWG URL §5.1 step 3.3). A query holding a field
/// [`url_decode`] refuses is answered 400 with that field and the reason.
pub(crate) fn parse_qs(query: &str) -> Result<Vec<(String, String)>, Response> {
    query
        .split('&')
        .filter(|field| !field.is_empty())
        .map(|field| {
            let (key, value) = match field.split_once('=') {
                Some(split) => split,
                None => (field, ""),
            };
            match (url_decode(key), url_decode(value)) {
                (Ok(key), Ok(value)) => Ok((key, value)),
                (Err(reason), _) | (_, Err(reason)) => Err(send_json(
                    http_status(reqwest::StatusCode::BAD_REQUEST),
                    &json!({"error": format!("the query field {field:?} cannot be read: {reason}")}),
                )),
            }
        })
        .collect()
}

pub(crate) fn query_value(values: &[(String, String)], name: &str) -> Option<String> {
    values
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
}

/// `input` percent-decoded with `+` as space, or why it cannot be: a `%` not
/// followed by two hexadecimal digits (RFC 3986 §2.1), or bytes that are not
/// UTF-8 once decoded.
pub(crate) fn url_decode(input: &str) -> Result<String, String> {
    let spaced = input.replace('+', " ");
    let mut pieces = spaced.split(PERCENT);
    let mut out: Vec<u8> = pieces.next().into_iter().flat_map(str::bytes).collect();
    let digits = PERCENT_ESCAPE.len() - PERCENT.len();
    for piece in pieces {
        let Some((hex, rest)) = piece.split_at_checked(digits) else {
            return Err(format!(
                "the escape {PERCENT}{piece:?} is not followed by {digits} hexadecimal digits"
            ));
        };
        let byte = u8::from_str_radix(hex, HEX_RADIX).map_err(|error| {
            format!("the escape {PERCENT}{hex} is not {digits} hexadecimal digits: {error}")
        })?;
        out.push(byte);
        out.extend_from_slice(rest.as_bytes());
    }
    String::from_utf8(out).map_err(|error| format!("its decoded bytes are not UTF-8: {error}"))
}

/// The percent sign that opens a percent-encoding (RFC 3986 §2.1,
/// <https://www.rfc-editor.org/rfc/rfc3986#section-2.1>).
const PERCENT: &str = "%";

/// One percent-encoding: `%` and two hexadecimal digits (RFC 3986 §2.1,
/// <https://www.rfc-editor.org/rfc/rfc3986#section-2.1>).
const PERCENT_ESCAPE: &str = "%HH";

/// The base of a percent-encoding's digits (RFC 3986 §2.1,
/// <https://www.rfc-editor.org/rfc/rfc3986#section-2.1>).
const HEX_RADIX: u32 = 16;
