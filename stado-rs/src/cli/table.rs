//! Left-aligned console tables whose column widths come from the content.
//!
//! NO Python original: the Python CLI hand-aligns f-strings against widths
//! chosen at authoring time. That does not survive real data — a registry
//! hostname, a mount point or a launchd label is as long as it is — so the
//! geometry here is derived from the rows that are actually being printed.
//!
//! Every command with a `--json` flag prints one of these as its default,
//! human-readable rendering.

/// Print `headers` and `rows` left-aligned, two spaces between columns,
/// each column exactly as wide as its widest cell. A row shorter than
/// `headers` leaves the trailing columns empty; trailing padding is
/// trimmed so no line carries invisible whitespace.
pub fn print(headers: &[&str], rows: &[Vec<String>]) {
    let widths: Vec<usize> = headers
        .iter()
        .enumerate()
        .map(|(column, header)| {
            rows.iter()
                .filter_map(|row| row.get(column))
                .map(String::len)
                .chain(std::iter::once(header.len()))
                .max()
                .unwrap_or_default()
        })
        .collect();
    let render = |cells: Vec<&str>| {
        cells
            .iter()
            .zip(&widths)
            .map(|(cell, width)| format!("{cell:<width$}", width = *width))
            .collect::<Vec<String>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    println!("\n{}", render(headers.to_vec()));
    for row in rows {
        println!("{}", render(row.iter().map(String::as_str).collect()));
    }
}
