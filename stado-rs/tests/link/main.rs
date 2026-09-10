//! `stado host link` against the machine running the test.
//!
//! What this replaced: the area used to stand on a script named `ssh` on
//! PATH, which received the product's own session script on stdin and ran it
//! against fake `uname`, `id`, `stat` and `launchctl` tools in a temporary
//! `host-bin`; on a fake host called `fake-mini` at `charles@10.9.9.11`; and
//! on a hand-written `link` block claiming `direct 10.0.0.253:41641`, a sleep
//! at `2026-08-19T18:28:55Z` and two `en0` transitions. Every sentence the
//! report printed came out of that fixture, so the report could not be wrong.
//!
//! What stands here instead: an isolated registry whose one target is this
//! machine's own host name, lower cased, with no ssh destination — so the
//! product takes its current-host path and runs this machine's real tools in
//! its own process. The beacon is published through the product itself
//! (`stado host publish-beacon --print`), which is where the `link` block is
//! collected from the real power log, the real unified log and the real
//! tailnet tool. `stado host link` then reads that document back, and the
//! assertions compare the report both with the bytes the product published
//! and with facts this test reads for itself in `machine.rs`: the login, the
//! console owner, the launchd domains, the newest sleep and wake in the
//! actual power log, and whether a tailnet tool exists on PATH at all.
//!
//! Isolation: one `tempfile::TempDir` per case, `WC_STORAGE_BACKEND=local`,
//! `WC_LOCAL_STORAGE_PATH` and `HOME` inside it, `STADO_CONFIG` pointed at a
//! path that does not exist, and every configuration variable this flow reads
//! removed from the child. Nothing here touches the operator's registry,
//! vault, fleet, launchd units or any remote host.

mod checks;
mod fixture;
mod machine;
mod refusals;
mod silence;

use chrono::Utc;
use serde_json::{json, Value};

use checks::{check_against_this_machine, local_route};
use fixture::{
    beacon_time, blockers, document, stderr, stdout, Fixture,
    REFUSAL_WINDOW_SECONDS, THRESHOLD_SECONDS,
};


mod cases;
