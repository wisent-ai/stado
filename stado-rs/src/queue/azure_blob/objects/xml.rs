//! The List Blobs response the walk feeds on, read by quick-xml into the
//! shape Azure documents (`EnumerationResults` > `Blobs` > `Blob` with its
//! `Properties` and `Metadata`, then `NextMarker`). An attribute, a namespace
//! prefix, a CDATA section or any entity reads as the value it carries, and
//! an answer that is not that document, a length that is not a number or a
//! time that is not an HTTP date refuses the page naming it instead of
//! listing a blob without the field.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::super::client::parse_http_date;
use super::listing::ListEntry;

#[derive(Deserialize)]
struct EnumerationResults {
    #[serde(rename = "Blobs")]
    blobs: Option<Blobs>,
    #[serde(rename = "NextMarker")]
    next_marker: Option<String>,
}

#[derive(Deserialize)]
struct Blobs {
    #[serde(rename = "Blob")]
    blob: Option<Vec<Blob>>,
}

#[derive(Deserialize)]
struct Blob {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Properties")]
    properties: Option<Properties>,
    #[serde(rename = "Metadata")]
    metadata: Option<BTreeMap<String, String>>,
}

#[derive(Deserialize)]
struct Properties {
    #[serde(rename = "Creation-Time")]
    creation_time: Option<String>,
    #[serde(rename = "Last-Modified")]
    last_modified: Option<String>,
    #[serde(rename = "Content-Length")]
    content_length: Option<u64>,
}

/// Parse a List Blobs response: the `<Blob>` entries plus the
/// `<NextMarker>` continuation (empty or absent on the final page).
pub(super) fn parse_list_blobs(xml: &str) -> Result<(Vec<ListEntry>, Option<String>), String> {
    let page: EnumerationResults = quick_xml::de::from_str(xml)
        .map_err(|error| format!("the List Blobs answer is not one: {error}"))?;
    let entries = page
        .blobs
        .and_then(|blobs| blobs.blob)
        .into_iter()
        .flatten()
        .map(entry)
        .collect::<Result<Vec<_>, String>>()?;
    Ok((entries, page.next_marker))
}

/// One listed blob as the walk reads it.
fn entry(blob: Blob) -> Result<ListEntry, String> {
    let (created, modified, size) = match blob.properties {
        Some(properties) => (properties.creation_time, properties.last_modified, properties.content_length),
        None => (None, None, None),
    };
    Ok(ListEntry {
        creation_time: stamp(&blob.name, "Creation-Time", created)?,
        last_modified: stamp(&blob.name, "Last-Modified", modified)?,
        size,
        metadata: blob.metadata.into_iter().flatten().collect(),
        name: blob.name,
    })
}

/// A blob's written time, read as the HTTP date Azure writes; one in another
/// shape is refused naming the blob, the field and the text.
fn stamp(name: &str, field: &str, written: Option<String>) -> Result<Option<DateTime<Utc>>, String> {
    written
        .map(|text| {
            parse_http_date(&text).ok_or_else(|| format!("blob {name}: {field} {text} is not an HTTP date"))
        })
        .transpose()
}
