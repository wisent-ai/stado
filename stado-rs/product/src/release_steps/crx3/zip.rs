//! The extension directory as the deflated zip a CRX3 file carries, with
//! `manifest.json` stamped to the release version. Entries are written in
//! path order with one fixed timestamp, so one tree always packs to one zip.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{bail, Context, Result};
use flate2::write::DeflateEncoder;
use flate2::{Compression, Crc};

/// Zip: signatures, the version a reader needs for deflate, the deflate method
/// and one fixed DOS timestamp (1980-01-01 00:00) so one tree packs to one zip.
const ZIP_LOCAL_HEADER: u32 = 0x0403_4b50;
const ZIP_CENTRAL_HEADER: u32 = 0x0201_4b50;
const ZIP_END_OF_DIRECTORY: u32 = 0x0605_4b50;
const ZIP_VERSION_DEFLATE: u16 = 20;
const ZIP_METHOD_DEFLATE: u16 = 8;
const ZIP_NO_FLAGS: u16 = 0;
const ZIP_DOS_TIME: u16 = 0;
const ZIP_DOS_DATE: u16 = (1 << 5) | 1;
const ZIP_EMPTY_U16: u16 = 0;
const ZIP_EMPTY_U32: u32 = 0;
/// Central directory: extra field length, comment length, disk number and
/// internal attributes, all empty.
const ZIP_EMPTY_CENTRAL_FIELDS: usize = 4;

/// Every file under `root`, as path components, in component order.
fn files(root: &Path, relative: &mut Vec<String>, out: &mut Vec<Vec<String>>) -> Result<()> {
    let directory = root.join(relative.join("/"));
    let mut entries = fs::read_dir(&directory)
        .with_context(|| format!("reading {}", directory.display()))?
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry
            .file_name()
            .into_string()
            .map_err(|name| anyhow::anyhow!("{name:?} is not UTF-8"))?;
        let kind = entry.file_type()?;
        relative.push(name);
        if kind.is_dir() {
            files(root, relative, out)?;
        } else if kind.is_file() {
            out.push(relative.clone());
        } else {
            bail!(
                "{} is neither a file nor a directory",
                entry.path().display()
            );
        }
        relative.pop();
    }
    Ok(())
}

/// The extension as a zip, `manifest.json` carrying `version`.
pub(super) fn extension(root: &Path, version: &str) -> Result<Vec<u8>> {
    let mut paths = Vec::new();
    files(root, &mut Vec::new(), &mut paths)?;
    let mut zip = Vec::new();
    let mut central = Vec::new();
    let mut count: u16 = 0;
    for components in &paths {
        let name = components.join("/");
        let mut bytes = fs::read(root.join(&name)).with_context(|| format!("reading {name}"))?;
        if name == "manifest.json" {
            let mut manifest: serde_json::Value =
                serde_json::from_slice(&bytes).context("manifest.json is not JSON")?;
            manifest["version"] = serde_json::Value::String(version.to_string());
            bytes = serde_json::to_vec_pretty(&manifest)?;
            bytes.push(b'\n');
        }
        let mut crc = Crc::new();
        crc.update(&bytes);
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&bytes)?;
        let compressed = encoder.finish()?;
        let offset = u32::try_from(zip.len()).context("the extension zip exceeds 4 GiB")?;
        let name_length = u16::try_from(name.len()).context("an extension path is too long")?;
        let sizes = [
            crc.sum(),
            u32::try_from(compressed.len())?,
            u32::try_from(bytes.len())?,
        ];
        zip.extend(ZIP_LOCAL_HEADER.to_le_bytes());
        for value in [
            ZIP_VERSION_DEFLATE,
            ZIP_NO_FLAGS,
            ZIP_METHOD_DEFLATE,
            ZIP_DOS_TIME,
            ZIP_DOS_DATE,
        ] {
            zip.extend(value.to_le_bytes());
        }
        sizes
            .iter()
            .for_each(|value| zip.extend(value.to_le_bytes()));
        zip.extend(name_length.to_le_bytes());
        zip.extend(ZIP_EMPTY_U16.to_le_bytes());
        zip.extend(name.as_bytes());
        zip.extend(&compressed);

        central.extend(ZIP_CENTRAL_HEADER.to_le_bytes());
        for value in [
            ZIP_VERSION_DEFLATE,
            ZIP_VERSION_DEFLATE,
            ZIP_NO_FLAGS,
            ZIP_METHOD_DEFLATE,
            ZIP_DOS_TIME,
            ZIP_DOS_DATE,
        ] {
            central.extend(value.to_le_bytes());
        }
        sizes
            .iter()
            .for_each(|value| central.extend(value.to_le_bytes()));
        central.extend(name_length.to_le_bytes());
        // Extra field, comment, disk number and internal attributes: none;
        // then no external attributes, then where the local header starts.
        for value in [ZIP_EMPTY_U16; ZIP_EMPTY_CENTRAL_FIELDS] {
            central.extend(value.to_le_bytes());
        }
        central.extend(ZIP_EMPTY_U32.to_le_bytes());
        central.extend(offset.to_le_bytes());
        central.extend(name.as_bytes());
        count = count
            .checked_add(1)
            .context("the extension has too many files")?;
    }
    let directory_offset = u32::try_from(zip.len())?;
    let directory_length = u32::try_from(central.len())?;
    zip.extend(central);
    zip.extend(ZIP_END_OF_DIRECTORY.to_le_bytes());
    zip.extend(ZIP_EMPTY_U16.to_le_bytes());
    zip.extend(ZIP_EMPTY_U16.to_le_bytes());
    zip.extend(count.to_le_bytes());
    zip.extend(count.to_le_bytes());
    zip.extend(directory_length.to_le_bytes());
    zip.extend(directory_offset.to_le_bytes());
    zip.extend(ZIP_EMPTY_U16.to_le_bytes());
    Ok(zip)
}
