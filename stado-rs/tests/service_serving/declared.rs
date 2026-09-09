//! Which port gets judged when the operator names none, and what the command
//! refuses to guess.

use serde_json::json;

use crate::fixture::{live_port, report, stderr, stdout, Fleet, SERVICE};

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
    assert_eq!(ports[0]["port"], json!(port), "{row}");
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
    assert_eq!(ports[0]["port"], json!(port), "{row}");
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
}
