//! The verdict about a port: who holds it, and whether this unit owns them.
//!
//! The defect that started this: a unit whose port is held by a process its
//! label does not own was reported as serving. The owner walk runs against
//! this machine's real `launchctl list` and this test process's real parent
//! chain — which is precisely the case that must come back `unknown` or
//! `served_by_other` rather than `serving`, because no launchd job owns a
//! `cargo test` process.

use serde_json::json;

use crate::fixture::{live_port, report, stderr, stdout, Fleet, SERVICE};

#[test]
fn a_port_held_by_a_process_this_unit_does_not_own_is_never_serving() {
    let fleet = Fleet::new();
    // Held by THIS test process. No launchd job owns a `cargo test` process,
    // so the owner walk cannot attribute it to the unit — which is exactly the
    // guarantee that stops one of two units with identical argv from claiming
    // the other's process.
    let (_held, port) = live_port();
    let out = fleet.serving(&["--port", &port.to_string(), "--json"]);
    let row = report(&out);

    assert_ne!(
        row["serving"], "serving",
        "a port this unit's label does not own was reported as serving:\n{row}"
    );
    let judged = &row["ports"][0];
    assert_eq!(judged["port"], json!(port));
    assert_ne!(judged["verdict"], "served_by_unit", "{row}");
    // The pid is named, because "not serving" without a pid is not actionable.
    let holder = &judged["holders"][0];
    let pid: u32 = holder["pid"].as_str().unwrap().parse().unwrap();
    assert_eq!(pid, std::process::id(), "the holder pid must be this test");
    assert!(
        !out.status.success(),
        "anything but `serving` must exit non-zero:\n{}",
        stdout(&out)
    );
}

#[test]
fn a_port_nothing_listens_on_is_not_serving_and_names_the_port() {
    let fleet = Fleet::new();
    // Bind, learn the port, then drop the listener: the port is real and
    // provably free rather than picked out of the air.
    let port = {
        let (listener, port) = live_port();
        drop(listener);
        port
    };
    let out = fleet.serving(&["--port", &port.to_string(), "--json"]);
    let row = report(&out);
    assert_eq!(row["serving"], "not_serving", "{row}");
    assert_eq!(row["ports"][0]["verdict"], "dead", "{row}");
    assert!(row["ports"][0]["holders"].as_array().unwrap().is_empty());
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains(&port.to_string()),
        "the failure must name the dead port:\n{}",
        stderr(&out)
    );
}

#[test]
fn a_resolved_foreign_owner_is_named_and_reported_as_not_declared() {
    let fleet = Fleet::new();
    let (_held, port) = live_port();
    let out = fleet.serving(&["--port", &port.to_string(), "--json"]);
    let row = report(&out);
    let holder = &row["ports"][0]["holders"][0];

    // The owner walk climbs this test process's real parent chain. Under a
    // terminal launched by a launchd application job it resolves that job;
    // under a bare shell it resolves nothing. Both answers are correct and the
    // guarantee is the same either way, so this defends the invariant rather
    // than the machine it happens to run on: the port is never credited to the
    // unit, and a resolved owner is named and judged against the registry.
    match holder["owner_state"].as_str().unwrap() {
        "resolved" => {
            let owner = holder["owner"].as_str().unwrap();
            assert!(!owner.is_empty(), "a resolved owner must be named:\n{row}");
            assert_ne!(owner, SERVICE, "{row}");
            // Registry knowledge is this side's, and this label is not in it.
            assert_eq!(holder["owner_declared"], json!(false), "{row}");
            assert_eq!(row["ports"][0]["verdict"], "served_by_other", "{row}");
            assert_eq!(row["serving"], "not_serving", "{row}");
            assert!(
                stderr(&out).contains(owner),
                "the failure must name the job that holds the port:\n{}",
                stderr(&out)
            );
        }
        "unknown" => {
            assert_eq!(holder["owner"], "", "{row}");
            assert_eq!(holder["owner_declared"], serde_json::Value::Null, "{row}");
            assert_eq!(row["ports"][0]["verdict"], "owner_unknown", "{row}");
            assert_eq!(row["serving"], "unknown", "{row}");
            assert!(
                stderr(&out).contains("could not be established"),
                "{}",
                stderr(&out)
            );
        }
        other => panic!("unexpected owner_state {other:?}:\n{row}"),
    }
    // Whatever the machine answered, this unit is not serving that port.
    assert_ne!(row["ports"][0]["verdict"], "served_by_unit", "{row}");
    assert_ne!(row["serving"], "serving", "{row}");
    assert!(!out.status.success());
}
