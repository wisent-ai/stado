//! `service file-fetch`: the byte-exact read `env-show` deliberately is not.
//!
//! Both cases write the source themselves and then compare the copy the
//! product left with the bytes on disk, plus a SHA-256 this test computes
//! rather than takes from the report.

use crate::{on_disk, report, sha256, stderr, stdout, Fleet};

/// The bytes `env-show` cannot report and this command must.
///
/// Shaped like the file that motivated the command:
/// `$HOME/.stado/bin/weles-release-cutover` is a shell script whose working
/// parts are a double-quoted `sed -E` program, a line continuation, and a tab.
/// `env-show`'s host-side sanitizer replaces every quote, every backslash and
/// every byte outside printable ASCII with `?`, so its report of this file
/// would not run.
const AWKWARD: &str = "#!/bin/bash\nset -euo pipefail\n\
                       /usr/bin/sed -E \"/^WC_SKARBIEC_URL=/d\" \\\n\
                       \t\"$HOME/.config/weles/worker.env\"\n\
                       # naïve — non-ASCII: café ✓\n";

/// Live operator tooling's mode: exactly what
/// `$HOME/.stado/bin/weles-release-cutover` carries on charless-mac-mini.
const EXECUTABLE_OWNER_ONLY: u32 = 0o700;

/// The mode a worker env file carries.
const OWNER_ONLY: u32 = 0o600;

#[test]
fn a_fetched_file_is_byte_exact_where_env_show_of_the_same_file_is_not() {
    let fleet = Fleet::new();
    let source = fleet.file(
        ".stado/bin/weles-release-cutover",
        AWKWARD.as_bytes(),
        EXECUTABLE_OWNER_ONLY,
    );
    let destination = fleet.destination("weles-release-cutover");

    let fetched = fleet.file_fetch(
        "$HOME/.stado/bin/weles-release-cutover",
        &["--dest-file", destination.to_str().unwrap()],
    );
    assert!(
        fetched.status.success(),
        "fetch failed: {}{}",
        stdout(&fetched),
        stderr(&fetched)
    );

    let original = on_disk(&source);
    assert_eq!(
        on_disk(&destination),
        original,
        "the copy is not byte-identical to the source"
    );
    // The digest is in the report, and it is the digest of these exact bytes.
    let table = stdout(&fetched);
    assert!(table.contains(&sha256(&original)), "{table}");
    assert!(table.contains("verified"), "{table}");

    // The gap this command closes, demonstrated rather than asserted about:
    // `env-show` is the only other reader of a file under a managed home, and
    // its report of this same file cannot reproduce it. Its sanitizer replaces
    // the quotes, the backslash and the non-ASCII bytes with `?`.
    let shown = fleet.env_show("$HOME/.stado/bin/weles-release-cutover");
    let described = stdout(&shown);
    assert!(
        described.contains('?'),
        "env-show reported no substitution, so this file needed no byte-exact reader:\n{described}"
    );
    assert!(
        !described.contains("café"),
        "env-show returned the source bytes verbatim:\n{described}"
    );

    // The mode the operator has to know before committing a launcher.
    let json = fleet.file_fetch(
        "$HOME/.stado/bin/weles-release-cutover",
        &["--json", "--dest-file", destination.to_str().unwrap()],
    );
    let row = report(&json);
    assert_eq!(row["file_state"], "read");
    assert_eq!(row["mode"], format!("{EXECUTABLE_OWNER_ONLY:o}"));
    assert_eq!(row["owner_only"], true);
    assert_eq!(row["integrity"], "verified");
    assert_eq!(row["host_digest"], row["local_digest"]);
    assert_eq!(row["host_digest"], serde_json::json!(sha256(&original)));
    assert_eq!(row["bytes"], serde_json::json!(original.len()));
    assert_eq!(row["fetched_bytes"], serde_json::json!(original.len()));
    // The second fetch rewrote the destination with the same bytes rather
    // than appending to the copy the first one left.
    assert_eq!(on_disk(&destination), original, "the rewrite doubled up");
    // The bytes are the destination file's business. A report an operator
    // pastes into a ticket must not be a second copy of the content.
    let text = serde_json::to_string(&row).unwrap();
    assert!(!text.contains("content"), "{text}");
    assert!(!text.contains("WC_SKARBIEC_URL"), "{text}");
}

#[test]
fn a_fetch_without_a_destination_reports_the_file_and_keeps_no_copy() {
    let fleet = Fleet::new();
    let source = fleet.file(
        ".config/weles/worker.env",
        b"WC_SKARBIEC_URL='http://127.0.0.1:8895'\n",
        OWNER_ONLY,
    );
    let out = fleet.file_fetch("$HOME/.config/weles/worker.env", &["--json"]);
    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let row = report(&out);
    let original = on_disk(&source);
    assert_eq!(row["integrity"], "verified");
    assert_eq!(row["dest_file"], "-");
    assert_eq!(row["bytes"], serde_json::json!(original.len()));
    // Both digests are the digest of the bytes this test wrote: the host
    // computed one with `shasum`, this process computed the other.
    assert_eq!(
        row["local_digest"],
        serde_json::json!(sha256(&original)),
        "the reported digest is not this file's"
    );
    assert_eq!(row["host_digest"], row["local_digest"]);
    // Nothing was left in the storage area a `--dest-file` would have used.
    assert!(
        !fleet.destination("worker.env").exists(),
        "a fetch with no destination still wrote one"
    );
}
