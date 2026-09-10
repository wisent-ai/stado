//! What the product says about a port: who owns it, what the declaration
//! names, and when it refuses to answer at all.
use super::*;

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
    assert_eq!(judged["port"], serde_json::json!(port));
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
            assert_eq!(holder["owner_declared"], serde_json::json!(false), "{row}");
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

#[test]
fn the_port_comes_from_the_declared_endpoint_when_none_is_named() {
    let fleet = Fleet::new();
    let (_held, port) = live_port();
    fleet.declare_endpoint(port);
    // No --port: the fleet's own declaration of what this service answers on
    // is what gets judged.
    let out = fleet.serving(&["--json"]);
    let row = report(&out);
    let ports = row["ports"].as_array().unwrap();
    assert_eq!(ports.len(), 1, "{row}");
    assert_eq!(ports[0]["port"], serde_json::json!(port), "{row}");
    let holder = &ports[0]["holders"][0];
    assert_eq!(
        holder["pid"].as_str().unwrap().parse::<u32>().unwrap(),
        std::process::id(),
        "{row}"
    );
    assert_ne!(row["serving"], "serving", "{row}");
}

/// The declared port must resolve when the directory and the host spell the
/// service differently, because that is how the fleet actually spells it.
///
/// `brama` is declared in the service directory as `brama` and runs on its
/// host as `com.wisent.always-on.brama`. This command took one `name` and used
/// it for both lookups: `declared_matching` against the host's labels and
/// `directory_port` against the directory's keys. Asked by service name it
/// refused with "is not a registry-managed service"; asked by label, with "the
/// service directory declares no endpoint ... name it with --port". So the one
/// service whose declared port was wrong was a service whose declared port
/// this command could not read, and on 2026-08-31 that port pointed at another
/// job for seventeen hours.
#[test]
fn the_declared_endpoint_resolves_when_the_directory_and_the_host_name_it_differently() {
    let fleet = Fleet::new();
    let (_held, port) = live_port();
    fleet.declare_endpoint_under_logical_name("weles", port);
    // Addressed by the launchd label, which is what the host declares, while
    // the endpoint is declared under the logical name.
    let out = fleet.serving(&["--json"]);
    assert!(
        !stderr(&out).contains("declares no endpoint"),
        "the declared endpoint must be found through the placement profile: {}",
        stderr(&out)
    );
    let row = report(&out);
    let ports = row["ports"].as_array().unwrap();
    assert_eq!(ports.len(), 1, "{row}");
    assert_eq!(ports[0]["port"], serde_json::json!(port), "{row}");
}

#[test]
fn naming_no_port_is_refused_rather_than_passed_as_an_empty_check() {
    let fleet = Fleet::new();
    // No --port and no declared endpoint: the command must refuse rather than
    // judge nothing and call it a pass.
    let out = fleet.serving(&["--json"]);
    assert!(!out.status.success());
    let said = format!("{}{}", stdout(&out), stderr(&out));
    assert!(said.contains("declares no endpoint"), "{said}");
    assert!(said.contains("--port"), "{said}");
}

#[test]
fn show_reports_what_the_unit_file_declares_and_does_not_call_it_running() {
    // The defect this capability exists for: `show` reaches no process table,
    // so its word must not be one an operator reads as "serving".
    let fleet = Fleet::new();
    let out = fleet.stado(&["service", "show", SERVICE, "--host", "here", "--json"]);
    let text = stdout(&out);
    let parsed: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("not JSON ({error}):\n{text}{}", stderr(&out)));
    let row = &parsed[0];
    assert_eq!(row["status"], "declares", "{row}");
    assert_ne!(row["status"], "runs", "{row}");
}
