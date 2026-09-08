//! What [`ProcessRow::parse`] must keep and what it must refuse.

use super::ProcessRow;

#[test]
fn row_keeps_the_whole_command_after_the_five_lstart_tokens() {
    let row = ProcessRow::parse(
        "38348 1 Tue Sep  1 16:35:37 2026 /u/.stado/bin/stado agent --target mini",
    )
    .expect("row parses");
    assert_eq!(row.pid, "38348");
    assert_eq!(row.ppid, "1");
    assert_eq!(row.started_at, "Tue Sep 1 16:35:37 2026");
    assert_eq!(row.command, "/u/.stado/bin/stado agent --target mini");
}

#[test]
fn a_row_missing_fields_is_dropped_rather_than_invented() {
    assert!(ProcessRow::parse("38348 1 Tue Sep").is_none());
}
