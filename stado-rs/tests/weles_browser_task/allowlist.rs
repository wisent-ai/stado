//! The gate: the action catalog the host itself carries, read off that host
//! byte-exactly and consulted before anything is enqueued.
//!
//! The registry row says a host MAY run an action. This file says the host's
//! worker WILL accept it. `host weles-capture` sends `generic_capture` to a
//! worker whose catalog does not carry it and the job is accepted and dropped,
//! so the refusal has to arrive here, naming the action and the host.

use serde_json::json;

use crate::fixture::{
    refusal, said, Fleet, ALLOWLIST_PATH, CAPTURE_ACTION, DEFAULT_ACTION, LOGIN_ACTION,
    SAVED_ACTION, TARGET,
};

/// A catalog long enough to break a reader that clamps: `service env-show`
/// reports at most 400 characters, and charless-mac-mini's real list is 4488.
/// The wanted action is the LAST line, so a clamped read refuses it.
fn long_catalog(last: &str) -> String {
    let mut body = String::new();
    for first in 'a'..='j' {
        for second in 'a'..='y' {
            body.push_str(&format!("filler_action_{first}{second}\n"));
        }
    }
    assert!(
        body.len() > 400,
        "the catalog must exceed env-show's clamp: {} bytes",
        body.len()
    );
    body.push_str(last);
    body.push('\n');
    body
}

#[test]
fn an_action_the_hosts_catalog_omits_is_refused_naming_it_the_host_and_what_does_exist() {
    let fleet = Fleet::declaring(&[DEFAULT_ACTION, CAPTURE_ACTION]);
    fleet.allowlist(&format!(
        "{DEFAULT_ACTION}\n{SAVED_ACTION}\n{LOGIN_ACTION}\n"
    ));
    let plan = fleet.plan(json!({ "action": CAPTURE_ACTION }));

    let out = fleet.run(Some(TARGET), &plan);

    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert_eq!(
        refusal(&out),
        format!(
            "{TARGET} does not accept the action \"{CAPTURE_ACTION}\": its \
             WELES_ACTION_ALLOWLIST carries 3 action(s) and that is not one of them, so the \
             worker would refuse the job. The general action(s) it does accept: \
             {DEFAULT_ACTION}, {SAVED_ACTION}"
        )
    );
    assert!(
        fleet.files_naming_the_session().is_empty(),
        "a refused action recorded the session: {:?}",
        fleet.files_naming_the_session()
    );
}

#[test]
fn a_host_with_no_catalog_file_is_refused_naming_the_path_it_looked_for() {
    let fleet = Fleet::declaring(&[DEFAULT_ACTION]);
    // No catalog written at all.
    let plan = fleet.plan(json!({}));

    let out = fleet.run(Some(TARGET), &plan);

    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert_eq!(
        refusal(&out),
        format!(
            "{TARGET}: could not read $HOME/{ALLOWLIST_PATH} to learn which actions this worker \
             accepts: missing (no regular file at the target)"
        )
    );
}

#[test]
fn a_catalog_that_lists_nothing_is_refused_rather_than_read_as_permissive() {
    let fleet = Fleet::declaring(&[DEFAULT_ACTION]);
    fleet.allowlist("\n\n");
    let plan = fleet.plan(json!({}));

    let out = fleet.run(Some(TARGET), &plan);

    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert_eq!(
        refusal(&out),
        format!(
            "{TARGET} declares no WELES_ACTION_ALLOWLIST, so no action can be shown to be \
             accepted there; the worker refuses every name outside that list"
        )
    );
}

/// The whole catalog is read, not its first 400 characters.
///
/// The proof is positive: with the wanted action on the last line of a 4-kB
/// file, the command must get PAST the gate and stop at the next thing, which
/// is reaching Weles. A clamped read would refuse a legitimate action instead.
#[test]
fn the_whole_catalog_is_read_so_an_action_on_its_last_line_still_passes_the_gate() {
    let fleet = Fleet::declaring(&[DEFAULT_ACTION]);
    fleet.allowlist(&long_catalog(DEFAULT_ACTION));
    let plan = fleet.plan(json!({}));

    let out = fleet.run(Some(TARGET), &plan);

    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert_eq!(
        refusal(&out),
        format!(
            "{TARGET}: the service directory carries no weles-admission entry, so nothing \
             declares where the Weles admission API listens"
        )
    );
    assert!(
        fleet.files_naming_the_session().is_empty(),
        "the run reached Weles and recorded the session: {:?}",
        fleet.files_naming_the_session()
    );
}

/// An older active release keeps its gate in one `WELES_ACTION_ALLOWLIST=`
/// assignment rather than one action per line, and must still be able to
/// explain itself.
#[test]
fn a_catalog_in_the_legacy_assignment_form_still_explains_the_gate() {
    let fleet = Fleet::declaring(&[DEFAULT_ACTION, CAPTURE_ACTION]);
    fleet.allowlist(&format!(
        "WELES_ACTION_ALLOWLIST={DEFAULT_ACTION},{SAVED_ACTION}\n"
    ));
    let plan = fleet.plan(json!({ "action": CAPTURE_ACTION }));

    let out = fleet.run(Some(TARGET), &plan);

    assert_eq!(out.status.code(), Some(1), "{}", said(&out));
    assert_eq!(
        refusal(&out),
        format!(
            "{TARGET} does not accept the action \"{CAPTURE_ACTION}\": its \
             WELES_ACTION_ALLOWLIST carries 2 action(s) and that is not one of them, so the \
             worker would refuse the job. The general action(s) it does accept: \
             {DEFAULT_ACTION}, {SAVED_ACTION}"
        )
    );
}
