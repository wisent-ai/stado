//! Having somewhere to record a copy at all, and what the product says when
//! it has not.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::fixture::{
    accepted_document, stderr, stdout, Fixture, CACHE_DOCUMENT, REFUSAL_PREFIX, TARGET,
};

/// A process with no `HOME` has nowhere to write a copy. It records nothing,
/// says nothing about it — there is no path to put in a sentence — and still
/// answers, because a daemon started with no environment reads the registry
/// too.
#[test]
fn a_host_with_no_cache_location_records_nothing_and_still_answers() {
    let fixture = Fixture::new();

    let output = fixture.stado_without_home(&["registry", "self"]);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains(TARGET), "{}", stdout(&output));
    assert!(
        !stderr(&output).contains(REFUSAL_PREFIX),
        "there is no path to name, so there is no sentence: {}",
        stderr(&output)
    );
    assert!(
        !holds_a_file_named(fixture.path(), CACHE_DOCUMENT),
        "no copy was written anywhere under the storage root"
    );
}

/// A cache location that cannot be written is named as such, with what the
/// disk said. The document already there is untouched, so the host keeps
/// answering from the copy it has instead of losing it to a failed write.
#[test]
fn a_cache_location_that_cannot_be_written_is_named_in_the_refusal() {
    let fixture = Fixture::new();
    fixture.read_registry();
    let recorded = fixture.recorded_copy().expect("a copy was recorded");
    std::fs::set_permissions(
        fixture.cache_directory(),
        std::fs::Permissions::from_mode(0o500),
    )
    .expect("seal the cache directory");

    // A document the contract accepts, so the only thing that can refuse the
    // refresh is the disk. Whitespace differs from the recorded copy, which is
    // what makes an overwrite detectable.
    fixture.publish(&accepted_document().replace("\"services\": []", "\"services\": [ ]"));
    let output = fixture.read_registry();

    std::fs::set_permissions(
        fixture.cache_directory(),
        std::fs::Permissions::from_mode(0o700),
    )
    .expect("reopen the cache directory so the storage root can be removed");
    let complaint = stderr(&output);
    assert!(
        complaint.contains(REFUSAL_PREFIX) && complaint.contains("Permission denied"),
        "the refusal names what the disk said: {complaint}"
    );
    assert!(
        output.status.success(),
        "the read the authority answered still answered: {complaint}"
    );
    assert_eq!(
        fixture.recorded_copy().as_deref(),
        Some(recorded.as_str()),
        "a failed write leaves the previous copy, never half of the next one"
    );
}

/// Whether any file called `name` exists anywhere under `root`.
fn holds_a_file_named(root: &Path, name: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(root) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        if path.is_dir() {
            return holds_a_file_named(&path, name);
        }
        path.file_name().is_some_and(|found| found == name)
    })
}
