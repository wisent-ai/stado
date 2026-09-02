//! The disk script must cost what its caller reads, and must not answer
//! differently when it costs less.
//!
//! `stado host gates lukasz-macbook`, run on `lukasz-macbook`, died with
//! `Command '['/bin/bash', '-s']' timed out after 120 seconds` after burning
//! `user 7m27s` of CPU, so `disk_cleanup_stalled` and
//! `cleanup_success_age_seconds` were unreadable on the machine the command
//! was running on. A gate condition nobody can read is the same as one that
//! does not exist.
//!
//! It was not a locality defect. `host_channel::run_script` already branches
//! on `target_is_this_host` (`host_channel.rs:639-658`, `:74-85`), and the
//! failing argv is the LOCAL `/bin/bash -s` with `UsedConnection::Local`. The
//! script was already running here; it could not finish inside
//! `remote_timeout`, because it computes what its caller throws away.
//!
//! The caller matrix for `host_disk::remote_script`:
//!
//! * `host_gates.rs:290` — `host gates` — reads `usage`, `state`, `snapshots`
//! * `host_disk.rs:551` — `host disk`, via `to_report:437` — reads all eight
//!   (`usage`, `state`, `snapshots`, `inventory`, `clone_summaries`,
//!   `lock_holders`, `lock_read`, `lock_path`)
//!
//! So the inventory has a real consumer and is kept; only the scope of the
//! work changes. `INVENTORY_SECTION`'s depth argument caps the OUTPUT and not
//! the traversal, so `du -xk -d 2 "$HOME"` walks the whole home tree: on this
//! host the three fields the gate reads take 0.8s together while the full
//! script had not finished after 180s.
//!
//! These tests defend the two properties that make this a fix rather than a
//! regression. The gate scope must actually drop the expensive work — a
//! scope that still walks `$HOME` fixes nothing. And every field the two
//! scopes share must be produced by the same bytes, because a cheap read
//! that can answer differently from the expensive one is a second
//! implementation of the same measurement, which is the class of defect this
//! whole line of work exists to stop.
//!
//! No host is contacted: the scripts are pure functions of crate constants.

use stado::deploy::host_disk::{remote_script, remote_script_for, DiskScope};

/// Markers only `host disk` consumes, each emitted by a section the gate
/// scope must not carry.
const INVENTORY_MARKERS: [&str; 3] = [
    "STADO_DISK_ITEM",
    "STADO_CLONE_SUMMARY",
    "STADO_CLEANUP_LOCK",
];

/// Markers `host gates` consumes. `assemble` reads `usage` from
/// `STADO_DISK`, `state` from `STADO_CLEANUP_STATE`, and `snapshots` from
/// `STADO_SNAPSHOT`.
const GATE_MARKERS: [&str; 3] = ["STADO_DISK", "STADO_CLEANUP_STATE", "STADO_SNAPSHOT"];

/// The commands that made the gate unanswerable on its own host. A gate
/// scope containing any of these has not fixed anything.
const EXPENSIVE_COMMANDS: [&str; 3] = ["/usr/bin/du", "/usr/bin/find", "/usr/sbin/lsof"];

/// The gate scope must carry every field the gate reads.
///
/// Without this the cheap script could be cheap by measuring nothing, which
/// is the same silence with a faster exit code.
#[test]
fn the_gate_scope_carries_every_field_the_gate_reads() {
    let gate = remote_script_for(DiskScope::GateInputs);
    for marker in GATE_MARKERS {
        assert!(
            gate.contains(marker),
            "gate scope must emit {marker}, which `assemble` reads:\n{gate}"
        );
    }
}

/// The gate scope must not carry the work only `host disk` reads.
///
/// This is the defect itself: 120 seconds of `du` for an `inventory` the gate
/// never looks at.
#[test]
fn the_gate_scope_drops_the_work_only_host_disk_reads() {
    let gate = remote_script_for(DiskScope::GateInputs);
    for marker in INVENTORY_MARKERS {
        assert!(
            !gate.contains(marker),
            "gate scope must not emit {marker}: no gate field reads it:\n{gate}"
        );
    }
    for command in EXPENSIVE_COMMANDS {
        assert!(
            !gate.contains(command),
            "gate scope must not run {command}; that is what timed out:\n{gate}"
        );
    }
}

/// `host disk` keeps everything. The capability is scoped, never removed.
#[test]
fn the_full_scope_still_carries_the_inventory() {
    let full = remote_script_for(DiskScope::Full);
    for marker in INVENTORY_MARKERS.iter().chain(GATE_MARKERS.iter()) {
        assert!(
            full.contains(marker),
            "full scope must emit {marker}: `to_report` reads it:\n{full}"
        );
    }
    assert!(
        full.contains("/usr/bin/du"),
        "the inventory is `host disk`'s own output and must survive"
    );
}

/// `remote_script` — the name both callers used before this change — must
/// still be the full script, so nothing that reads the inventory changes.
#[test]
fn remote_script_is_still_the_full_script() {
    assert_eq!(
        remote_script(),
        remote_script_for(DiskScope::Full),
        "remote_script() must remain the every-field script"
    );
}

/// The property that makes the cheap path safe: the gate script is the full
/// script with whole sections removed, never a rewrite of the shared ones.
///
/// Every line of the gate scope must appear in the full scope, in order.
/// If it does, the two cannot disagree on a field they both keep, because
/// the bytes producing it are the same bytes. If someone later hand-edits a
/// shared section for one scope only, this fails.
#[test]
fn the_gate_script_is_a_subsequence_of_the_full_script() {
    let full = remote_script_for(DiskScope::Full);
    let gate = remote_script_for(DiskScope::GateInputs);
    let mut remaining = full.lines();
    for line in gate.lines() {
        assert!(
            remaining.any(|candidate| candidate == line),
            "gate line is absent from the full script, so the scopes have \
             diverged on a shared section: {line:?}"
        );
    }
}

/// Stronger than the subsequence: deleting exactly the two omitted sections
/// from the full script yields the gate script byte for byte.
///
/// This is the identity claim stated as an equality rather than as prose.
/// `usage`, `state` and `snapshots` are emitted by the same text under both
/// scopes, so the gate cannot read a different free-space figure, a
/// different `last_success_at`, or a different snapshot list than `host
/// disk` would have read at the same moment.
#[test]
fn the_shared_sections_are_byte_identical_between_scopes() {
    let full = remote_script_for(DiskScope::Full);
    let gate = remote_script_for(DiskScope::GateInputs);

    // The lock section opens at its `lock=` assignment and ends with the
    // `fi` after its marker; the inventory opens at the Darwin guard and
    // runs to the end. Both are whole `if` blocks, so removing them leaves
    // valid shell.
    let lock_start = full
        .find("lock=\"$HOME/")
        .expect("full carries the lock section");
    let lock_end = full
        .find("if [ -x /usr/bin/tmutil ]; then")
        .expect("full carries the snapshot section");
    let inventory_start = full
        .find("if [ \"$(/usr/bin/uname")
        .expect("full carries the inventory section");

    let mut trimmed = String::with_capacity(full.len());
    trimmed.push_str(&full[..lock_start]);
    trimmed.push_str(&full[lock_end..inventory_start]);

    assert_eq!(
        trimmed, gate,
        "the gate script must be exactly the full script minus the lock and \
         inventory sections; anything else means a shared measurement was \
         re-spelled for one scope"
    );
}

/// Both scopes must keep the `set -u` header, and the gate script must still
/// be a complete program rather than a fragment.
#[test]
fn both_scopes_are_whole_programs() {
    for scope in [DiskScope::Full, DiskScope::GateInputs] {
        let script = remote_script_for(scope);
        assert!(
            script.starts_with("set -u\n"),
            "{scope:?} must open with the same shell header"
        );
        assert_eq!(
            script.matches("if [ -r \"$state\" ]; then").count(),
            1,
            "{scope:?} must read the janitor state file exactly once"
        );
        // No substitution marker may survive into a script that gets run.
        for marker in ["@STATE_PATH@", "@LOCK_PATH@"] {
            assert!(
                !script.contains(marker),
                "{scope:?} left {marker} unsubstituted"
            );
        }
    }
}
