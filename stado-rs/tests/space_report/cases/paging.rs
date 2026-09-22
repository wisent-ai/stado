//! The paging line, in the units the reading actually carries.

use crate::fixture::Fixture;

/// The compressor and the lifetime swapouts are read on every pass, and
/// until 2026-09-19 they were printed nowhere: charless-mac-mini reported
/// 4487 MiB available and swap 71%, both inside their watermarks, while its
/// compressor held 3.5 GiB and the released Brama was quarantined twice in
/// one hour for a readiness probe it could not answer. The text now carries
/// what the reading carries, converted with the host's own page size.
#[test]
fn the_paging_line_says_exactly_what_the_reading_measured() {
    let fixture = Fixture::new();
    let document: serde_json::Value =
        serde_json::from_slice(&fixture.report("0", &["--json"]).stdout)
            .expect("the report is one JSON document");
    let reading = &document["memory_reclaim"]["reading"];
    let text = String::from_utf8_lossy(&fixture.report("0", &[]).stdout).into_owned();
    let compressor = reading["compressor_pages"].as_i64();
    let swapouts = reading["swapouts"].as_i64();
    if compressor.is_none() && swapouts.is_none() {
        assert!(
            !text.contains("memory paging:"),
            "a host that measured neither number got a paging line anyway: {text}"
        );
        fixture.cleanup();
        return;
    }
    assert!(
        text.contains("memory paging:"),
        "the host measured its paging and the report dropped it: {text}"
    );
    if let Some(pages) = compressor {
        let rendered = match reading["page_size_bytes"].as_i64() {
            Some(size) => format!(
                "compressor holds {} MiB",
                pages.saturating_mul(size) / (1024 * 1024)
            ),
            None => format!("compressor holds {pages} pages"),
        };
        assert!(
            text.contains(&rendered),
            "the line does not match the reading ({rendered}): {text}"
        );
    }
    if let Some(swapouts) = swapouts {
        assert!(
            text.contains(&format!("{swapouts} swapout(s) since boot")),
            "the swapout count was dropped: {text}"
        );
    }
    fixture.cleanup();
}
