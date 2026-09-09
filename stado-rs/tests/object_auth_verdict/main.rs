//! What the product reports when an object request is not authorized.
//!
//! # What this area used to be
//!
//! The area existed for one distinction: a store that refused the question, or
//! could not answer it, must never be reported as a store that answered "not
//! there". It guarded that by calling `doctor::object_auth_verdict` with four
//! hand-built `SkarbiecError` values and reading the `Check` it returned — no
//! socket, no store, no command, no exit code. A product that had stopped
//! reporting refusals anywhere an operator can see them would have passed it.
//!
//! # What it is now
//!
//! Each case binds a real gateway on loopback ([`gateway::Gateway`]), declares
//! it as the store the way an operator declares one — `WC_STADO_STORAGE_URL`,
//! an owner-only `WC_STADO_STORAGE_TOKEN_FILE`, `WC_STADO_STORAGE_NAMESPACE` —
//! and drives the command that asks about an object. A `401` from a socket is
//! a real transport carrying a real refusal, so what the binary prints and the
//! code it exits with are the product's actual answer.
//!
//! The distinction is asserted on every surface that has one: `stat` reports
//! `refused` and exits non-zero where an answered absence reports `absent` and
//! exits zero, `cat` says REFUSED rather than absent, `get` writes no file,
//! and a refused listing is an error rather than an empty list. `doctor.rs`
//! covers the row `object_auth_verdict` itself builds.

mod doctor;
mod fixture;
mod gateway;

use fixture::{printed, said, Store, OBJECT_KEY};
use gateway::REFUSAL_BODY;

/// The three answers a gateway gives about one object: this reader may not
/// ask, the object is not there, and the reader may not ask again with a
/// different word for it.
const UNAUTHORIZED: u16 = 401;
const FORBIDDEN: u16 = 403;
const NOT_FOUND: u16 = 404;

/// The verdict words `stado storage stat` reports, and its exit-code
/// contract: zero means the question was ANSWERED, non-zero means it was not.
const REFUSED: &str = "refused";
const ABSENT: &str = "absent";
const ANSWERED_EXIT: i32 = 0;
const UNANSWERED_EXIT: i32 = 1;

/// The sentence `stat` leaves with when the store refused, copied from a live
/// run of the built binary against this fixture's own gateway.
const REFUSED_SENTENCE: &str = "is REFUSED, not absent — the store answered that this reader may \
                                not ask: repair the credential or the grant, because the same \
                                question asked again cannot learn anything";
/// And the sentence it prints when the store answered that the object is gone.
const ANSWERED_SENTENCE: &str = "The store ANSWERED: \"registry.json\" is not there. This is not \
                                 the same as a store that refused the question, could not answer \
                                 it now, or could not be reached at all.";

/// A refusal is reported as a refusal, with the status that produced it, and
/// the question is reported as unanswered.
///
/// Both statuses a gateway refuses with are driven, because the area exists to
/// keep them out of the `absent` bucket and one of them passing is not the
/// claim.
#[test]
fn a_refused_object_is_reported_refused_and_never_absent() {
    for status in [UNAUTHORIZED, FORBIDDEN] {
        let store = Store::answering(status);
        let output = store.run(&["storage", "stat", OBJECT_KEY, "--json"]);

        assert_eq!(
            output.status.code(),
            Some(UNANSWERED_EXIT),
            "HTTP {status} left exit {:?}; a store that refused the question did \
             not answer it\nstderr:\n{}",
            output.status.code(),
            said(&output.stderr)
        );
        let receipt = printed(&output);
        assert_eq!(
            receipt["state"], REFUSED,
            "HTTP {status} was reported as {}",
            receipt["state"]
        );
        assert_eq!(receipt["backend"], "stado");
        assert_eq!(receipt["path"], OBJECT_KEY);
        let detail = receipt["detail"]
            .as_str()
            .unwrap_or_else(|| panic!("a refusal carries its detail: {receipt}"));
        assert!(
            detail.contains(&status.to_string()) && detail.contains(REFUSAL_BODY),
            "the receipt does not carry what the gateway said: {detail}"
        );
        assert!(
            receipt["size"].is_null() && receipt["version"].is_null(),
            "nothing is known about a refused object: {receipt}"
        );

        let stderr = said(&output.stderr);
        assert!(
            stderr.contains(REFUSED_SENTENCE),
            "the refusal did not say what a reader must do about it:\n{stderr}"
        );
        assert!(
            stderr.contains("Treat the object's existence as unknown."),
            "the refusal did not say the object's existence is unknown:\n{stderr}"
        );
        assert!(
            store.gateway.object_requests() > 0,
            "the verdict was reported without asking the store anything"
        );
    }
}

/// An answered absence keeps its own verdict and its own exit code, which is
/// what makes the refusals above mean something.
#[test]
fn an_answered_absence_is_reported_absent_and_the_question_counts_as_answered() {
    let store = Store::answering(NOT_FOUND);
    let output = store.run(&["storage", "stat", OBJECT_KEY, "--json"]);

    assert_eq!(
        output.status.code(),
        Some(ANSWERED_EXIT),
        "a store that answered was reported as unanswered\nstderr:\n{}",
        said(&output.stderr)
    );
    let receipt = printed(&output);
    assert_eq!(receipt["state"], ABSENT);
    assert!(
        receipt["detail"].is_null(),
        "an answered absence needs no detail: {receipt}"
    );

    // The table form is what an operator reads, and it says in words that this
    // is not one of the three silences.
    let table = store.run(&["storage", "stat", OBJECT_KEY]);
    assert_eq!(table.status.code(), Some(ANSWERED_EXIT));
    let printed_table = said(&table.stdout);
    assert!(
        printed_table.contains(ANSWERED_SENTENCE),
        "the answered absence did not distinguish itself from a silence:\n{printed_table}"
    );
}

/// A refused download leaves no file behind, and says which of the two it was.
///
/// The destination is the assertion: a refusal that wrote an empty file would
/// be indistinguishable from an object that is empty, and every later reader
/// of that path would be reading a refusal as content.
#[test]
fn a_refused_download_writes_no_file_and_an_absent_one_says_so() {
    for status in [UNAUTHORIZED, FORBIDDEN] {
        let store = Store::answering(status);
        let destination = store.destination();
        let output = store.run(&[
            "storage",
            "get",
            &store.uri(),
            &destination.to_string_lossy(),
        ]);

        assert_eq!(
            output.status.code(),
            Some(UNANSWERED_EXIT),
            "HTTP {status} left exit {:?}\nstderr:\n{}",
            output.status.code(),
            said(&output.stderr)
        );
        assert!(
            !destination.exists(),
            "a refused download wrote {}",
            destination.display()
        );
        let stderr = said(&output.stderr);
        assert!(
            stderr.contains(&format!("HTTP {status}")) && stderr.contains(gateway::OBJECT_ROUTE),
            "the refused download did not name the route it asked and the \
             answer it got:\n{stderr}"
        );
        assert!(
            !stderr.contains(": absent"),
            "a refused download was reported as an absence:\n{stderr}"
        );
    }

    // The same command against a store that answered names the absence
    // instead, and equally writes nothing.
    let store = Store::answering(NOT_FOUND);
    let destination = store.destination();
    let output = store.run(&[
        "storage",
        "get",
        &store.uri(),
        &destination.to_string_lossy(),
    ]);
    assert_eq!(output.status.code(), Some(UNANSWERED_EXIT));
    assert!(
        !destination.exists(),
        "a download of an absent object wrote {}",
        destination.display()
    );
    let stderr = said(&output.stderr);
    assert!(
        stderr.contains(&format!("HTTP {NOT_FOUND}")),
        "the absent download did not name the gateway's answer:\n{stderr}"
    );
}

/// A refused listing is an error, not an empty namespace.
///
/// This is the same defect one surface along: a reader that printed
/// `{"objects": []}` for a namespace it was refused would report an
/// authorization boundary as a drained store, and a caller deciding whether a
/// coordinate is spent would believe it.
#[test]
fn a_refused_listing_is_not_an_empty_namespace() {
    let store = Store::answering(UNAUTHORIZED);
    let output = store.run(&["storage", "objects", "images", "--json"]);

    assert_eq!(
        output.status.code(),
        Some(UNANSWERED_EXIT),
        "a refused listing exited {:?}\nstderr:\n{}",
        output.status.code(),
        said(&output.stderr)
    );
    let stdout = said(&output.stdout);
    assert!(
        !stdout.contains("objects"),
        "a refused listing printed a listing:\n{stdout}"
    );
    let stderr = said(&output.stderr);
    assert!(
        stderr.contains(&format!("HTTP {UNAUTHORIZED}")) && stderr.contains(gateway::LIST_ROUTE),
        "the refused listing did not name the route and the answer:\n{stderr}"
    );

    // `cat` is the third reader of the same object and keeps the same
    // distinction: a refusal is never an empty body.
    let body = store.run(&["storage", "cat", OBJECT_KEY]);
    assert_eq!(body.status.code(), Some(UNANSWERED_EXIT));
    assert!(
        body.stdout.is_empty(),
        "a refused read wrote {} bytes to stdout",
        body.stdout.len()
    );
    assert!(
        said(&body.stderr).contains(&format!("HTTP {UNAUTHORIZED}")),
        "the refused read did not name the gateway's answer:\n{}",
        said(&body.stderr)
    );
}
