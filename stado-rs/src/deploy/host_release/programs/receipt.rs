//! The record a delivery leaves beside the artefact it installed.

/// The receipt, beside the artefact it describes. Appended by both activate
/// programs, the single-file one and the tree one.
///
/// `converge` can already prove WHETHER these bytes were delivered, by
/// comparing the installed file against this staged copy. It could not say
/// WHO, and on 2026-08-31 that is exactly where an investigation stopped: a
/// `stado` answering 0.13.19 appeared in `$HOME/.stado/bin` on the always-on
/// Mac at 21:25Z, the release channel was ruled out, both operator sessions
/// were ruled out, the repository's automation was ruled out, and nothing on
/// the host recorded who had installed it.
///
/// It lives in the version/platform directory rather than beside the active
/// binary, so a receipt cannot outlive the artefact it describes or be read
/// for a different one. A delivery made before this format simply has none,
/// and its absence means "installed before receipts", never "suspicious":
/// `staged-match` and `no-staged-copy` answer correctly without it.
///
/// Two digests, because they answer two questions. `sha256` is the archive
/// the manifest named and the delivery verified on the way in; on its own it
/// cannot be matched against anything a host carries, which is why
/// `stado release provenance` reported every delivered binary as accounted
/// for by nothing — the digest in the receipt described a tarball nobody
/// keeps. `artifact_sha256` is the installed file this receipt sits beside,
/// so a reader can prove the record describes the bytes in place. The tree
/// shape installs a directory and leaves it empty.
///
/// One copy: this text and its `printf` existed twice, once per activate
/// program, byte-identical, and a second copy is how a field added for one
/// shape silently misses the other.
pub const RECEIPT_BODY: &str = r##"
receipt_dir="$stado_home/releases/$binary/$version/$platform"
if [ -d "$receipt_dir" ]; then
  printf '{"binary":"%s","version":"%s","platform":"%s","source_commit":"%s","sha256":"%s","artifact_sha256":"%s","installed_at":"%s","delivered_by":"%s"}\n' \
    "$binary" "$version" "$platform" "${source_commit:-}" "${expected_sha256:-}" \
    "${active_sha256:-}" "$(/bin/date -u +%Y-%m-%dT%H:%M:%SZ)" "${delivered_by:-}" \
    > "$receipt_dir/release-receipt.json" 2>/dev/null || true
  /bin/chmod 600 "$receipt_dir/release-receipt.json" 2>/dev/null || true
fi
"##;

/// The marker that closes the activate step, printed after the receipt so a
/// reader of the transcript cannot see `step activate` before the record of
/// what was activated exists.
pub const ACTIVATE_STEP_MARKER: &str = "say step activate\n";
