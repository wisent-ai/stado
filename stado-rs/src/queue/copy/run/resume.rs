//! Turning the persisted cursor into the prefixes a run still has to walk.

/// Split the selected prefixes at the resume cursor: the cursor that was
/// consumed (empty when the run starts from the top) and the prefixes still
/// to walk.
///
/// An exhausted tail restarts from the top. The cursor is a within-run
/// resume point, NOT a "done" flag: the pre-cutover re-sync has to walk
/// every prefix again to catch churn, and object-level skipping keeps that
/// cheap.
pub(super) fn resume_split<'a>(
    prefixes: &'a [String],
    cursor: &str,
    full_run: bool,
) -> (String, &'a [String]) {
    let Some(index) = resume_index(prefixes, cursor, full_run) else {
        return (String::new(), prefixes);
    };
    // `split_first` drops the cursor prefix itself: it finished cleanly.
    match prefixes[index..].split_first() {
        Some((done, rest)) if !rest.is_empty() => (done.clone(), rest),
        _ => (String::new(), prefixes),
    }
}

/// Index of the last prefix the resume cursor covers.
///
/// The cursor is only honored on a full canonical run: when the operator
/// names prefixes explicitly they asked for exactly those, and silently
/// fast-forwarding past them would be a trap. Per-object skipping keeps the
/// restricted re-run cheap anyway.
fn resume_index(prefixes: &[String], cursor: &str, full_run: bool) -> Option<usize> {
    if !full_run || cursor.is_empty() {
        return None;
    }
    prefixes.iter().position(|prefix| prefix == cursor)
}
