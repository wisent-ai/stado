//! Reader-side registry cache tests against the local storage backend.
//!
//! Every test drives the built `stado` binary (`CARGO_BIN_EXE_stado`) with
//! WC_STORAGE_BACKEND=local + WC_LOCAL_STORAGE_PATH=<TempDir> and a
//! STADO_CONFIG pointing at a nonexistent path, so the developer's real
//! config can never leak into a test.
//!
//! HOME is a tempdir too, and that one is not optional: the last-known-good
//! copy lives at `$HOME/.stado/cache/registry-last-good.json`, so a test that
//! isolates only the storage backend writes the operator's real cache and a
//! later outage serves the fleet a toy registry. That happened on
//! 2026-08-19 — the live copy was found holding a two-line fake document with
//! one target — which is why the paths below are spelled out literally rather
//! than read back from the code under test.
//!
//! What is defended here: a successful canonical read records the copy and
//! dates it, an unreachable authority serves that copy and names its age in
//! the sentence the operator sees, a document that fails the registry-v2
//! contract is never recorded, and the snapshot bundled with the binary is
//! reached only when the authority AND the copy are both gone.

mod support;

use support::{
    read, stderr, stdout, Fleet, CONTRACT_VIOLATING_REGISTRY, EMPTY_FLEET_REGISTRY,
    SEEDED_REGISTRY, SEEDED_REGISTRY_GROWN, UNREACHABLE_AUTHORITY,
};


mod cases;
