//! The janitor's journal must tell a refused registry apart from a broken one.
//!
//! `resolve_canonical_policy` gates on `validate_registry`, which rejects a
//! registry declaring anything the running build has no implementation for.
//! Between 2026-08-20 and 2026-09-02 that gate fired 8348 times on
//! `lukasz-macbook` in two windows, and every one of them was journalled as
//! `policy:ValueError` — the same entry a corrupt document produces. The
//! document was never corrupt. It was valid, and the process holding it was
//! older than it. Neither window opened on a restart and both closed on one,
//! so the only signal an operator had said "invalid registry" while the actual
//! fault was the age of the running binary.
//!
//! What is defended here, all of it read off a real run of the built binary on
//! the machine running this test: a validation refusal is journalled
//! `policy:NotImplementedError` and deletes nothing; a document that does not
//! parse is STILL journalled `policy:ValueError`; the entry stays
//! `<stage>:<code>` with no field path, message text or version in it; and the
//! whole verdict survives into the janitor's own state file, because the state
//! file is what `stado space report` reads back to an operator.
//!
//! The fourth case is the other half of the same lesson and it is new. An
//! unknown cleaner NAME is no longer a refusal at all: `validation_disk` skips
//! it and reports it as `unknown_cleaners`, because refusing the whole policy
//! for one unfamiliar name switched every cleaner off on charless-mac-mini on
//! 2026-09-04 the moment `release_store` was declared for a binary still
//! queued to reach it. A registry is one document read by every release in the
//! fleet at once, and the older readers must keep cleaning.
//!
//! WHAT THIS AREA USED TO DO, and no longer does. It called
//! `resolve_canonical_policy` and `CleanupReport::add_error` directly and
//! never ran the product: no command, no store, no disk, no persisted state.
//! Three of its six cases asserted that an unknown cleaner name is refused,
//! which this build deliberately stopped doing — they were failing on
//! `origin/main` before this rewrite, pinning behaviour the product had
//! removed on purpose.
//!
//! DELETED, with the reason. `the_fixture_registry_resolves` asserted the
//! return value of `resolve_canonical_policy` on the accepted document —
//! target name, mode, digest length, defaulted flag — none of which an
//! operator can see anywhere. Its job, proving the fixture is not what makes a
//! refusal, is done by
//! `cases::an_unknown_cleaner_name_is_skipped_and_the_declared_cleaner_runs`,
//! which runs the same document through the real command and watches it delete
//! a real directory. `a_refusal_and_a_parse_failure_do_not_share_an_entry`
//! asserted only that two journal entries differ; both cases below assert
//! their exact entry, which says the same thing and says which.

mod cases;
mod fixture;
