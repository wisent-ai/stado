//! The List Blobs response parse the walk feeds on.
//!
//! The payload is read with plain tag scans rather than a parser: inside
//! `<Blobs>` it carries no attributes, and the only escaping in it is the
//! five predefined entities.

use std::collections::BTreeMap;

use super::super::client::parse_http_date;
use super::listing::ListEntry;

/// Text content of the first `<tag>...</tag>` in `xml` (plain tags only —
/// the List Blobs payload carries no attributes inside `<Blobs>`).
fn xml_tag<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find(&close)? + start;
    Some(&xml[start..end])
}

/// Decode the five predefined XML entities (`&amp;` last so an escaped
/// ampersand in front of another entity's name stays literal).
fn xml_unescape(raw: &str) -> String {
    raw.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Parse the `<Metadata>` children of one blob block.
fn parse_metadata(block: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(mut rest) = xml_tag(block, "Metadata") else {
        return out;
    };
    while let Some(open_end) = rest.find('>') {
        let tag = &rest[1..open_end];
        if tag.is_empty() || tag.contains(['<', '/', ' ']) {
            break;
        }
        let close = format!("</{tag}>");
        let Some(close_start) = rest.find(&close) else {
            break;
        };
        let value = &rest[open_end + 1..close_start];
        out.insert(tag.to_string(), xml_unescape(value));
        rest = &rest[close_start + close.len()..];
        if !rest.starts_with('<') {
            break;
        }
    }
    out
}

/// Parse a List Blobs response: the `<Blob>` entries plus the
/// `<NextMarker>` continuation (empty/absent = final page).
pub(super) fn parse_list_blobs(xml: &str) -> (Vec<ListEntry>, Option<String>) {
    let mut entries = Vec::new();
    let mut rest = xml;
    while let Some(blob_start) = rest.find("<Blob>") {
        let after_open = &rest[blob_start + "<Blob>".len()..];
        let Some(blob_end) = after_open.find("</Blob>") else {
            break;
        };
        let block = &after_open[..blob_end];
        entries.push(ListEntry {
            name: xml_tag(block, "Name").map(xml_unescape).unwrap_or_default(),
            creation_time: xml_tag(block, "Creation-Time").and_then(parse_http_date),
            last_modified: xml_tag(block, "Last-Modified").and_then(parse_http_date),
            size: xml_tag(block, "Content-Length").and_then(|value| value.parse().ok()),
            metadata: parse_metadata(block),
        });
        rest = &after_open[blob_end + "</Blob>".len()..];
    }
    let next = xml_tag(rest, "NextMarker").map(xml_unescape);
    (entries, next)
}
