//! The three readings a public-origin verdict is composed from, each taken
//! against this machine.
//!
//! `stado web origin status` asks a public resolver about the declared name,
//! asks the declared target what it publishes, and asks the deployment's own
//! control origin which origin the public edge selected. The first two are
//! this host: the name is the kernel's own and the publication is read out of
//! this node's tailscale handler table. The third is a connection, and both
//! of its real outcomes are exercised — one against the product's own HTTP
//! service running on this machine's loopback, one against a loopback port
//! nothing is listening on.

use serde_json::{json, Value};

use crate::fixture::{self, only_row, report, stderr, Fixture, ORIGIN, PUBLISHED_PATH, TARGET};
use crate::listeners::{accepts, free_port, Service, Upstream};

/// The route the deployment publishes the origin it selected on, spelled the
/// way the product spells it. A declared name copied from a live run, not
/// constants, config or tuning.
const SELECTION_PATH: &str = "/api/release/origin";

/// A registry declaring this machine, publishing this machine's own name,
/// forwarding to an upstream the caller keeps bound for the whole case.
fn declared(fixture: &Fixture, upstream: &Upstream) -> String {
    let hostname = fixture::hostname();
    let mut document = fixture::current_host_registry();
    document["public_origins"] = fixture::origin_row(&hostname, TARGET, &upstream.origin());
    let pushed = fixture.push(&document);
    assert!(pushed.status.success(), "{}", stderr(&pushed));
    hostname
}

fn status(fixture: &Fixture, api_url: &str) -> std::process::Output {
    fixture.stado_with(
        &[("STADO_API_URL", api_url)],
        &["web", "origin", "status", "--json"],
    )
}

/// What the report says about a name that is real on this network and absent
/// from every public one. The verdict is the resolution's, because a name no
/// client outside this deployment can find makes every later reading
/// unactionable.
fn assert_not_public(row: &Value, hostname: &str) {
    assert_eq!(row["name"], json!(ORIGIN));
    assert_eq!(row["verdict"], json!("origin-not-public"), "{row:#}");
    assert_eq!(row["resolution"]["state"], json!("dns_unresolved"));
    assert_eq!(row["resolution"]["hostname"], json!(hostname));
    assert_eq!(
        row["origin_error"],
        json!(format!(
            "{hostname} has no public A or AAAA record, so nothing outside this deployment's own \
             network can reach that origin, whatever it is serving"
        ))
    );
}

/// A control origin that answers, driven against the product's own service.
///
/// `stado dashboard` serves this fleet's control routes and does not serve the
/// selection route, so a real request to it comes back HTTP 404 with no
/// selected origin — and that is what the report has to say, naming the
/// endpoint it asked. The case then stops the service and shows the port
/// stopped accepting, so nothing is left listening.
#[test]
fn origin_status_reports_what_this_machines_own_stado_service_answered() {
    let fixture = Fixture::new();
    let upstream = Upstream::bind();
    let hostname = declared(&fixture, &upstream);
    let mut service = Service::start(&fixture);
    let port = service.port();

    let reported = status(&fixture, &service.url());
    assert_eq!(
        reported.status.code(),
        Some(1),
        "an origin that is not serving must exit non-zero: {}",
        stderr(&reported)
    );
    let rows = report(&reported);
    let row = only_row(&rows);
    assert_not_public(row, &hostname);

    let edge = &row["edge_selection"];
    assert_eq!(
        edge["endpoint"],
        json!(format!("http://127.0.0.1:{port}{SELECTION_PATH}")),
        "the report must name the endpoint it asked"
    );
    assert_eq!(edge["state"], json!("unreadable"));
    assert_eq!(edge["origin"], Value::Null);
    let detail = edge["detail"]
        .as_str()
        .expect("the edge reading has a detail");
    assert!(
        detail.starts_with("the public edge answered HTTP ")
            && detail.contains("named no selected origin"),
        "a service that answered must be reported as having answered: {detail}"
    );
    assert!(
        stderr(&reported).contains(&format!("not serving: {ORIGIN}: origin-not-public")),
        "the failing origin must be named on stderr: {}",
        stderr(&reported)
    );

    service.stop();
    assert!(
        !accepts(port),
        "the service this case started is still listening on 127.0.0.1:{port}"
    );
}

/// A control origin that refuses the connection. Nothing is bound on the port,
/// which is a different fact from a service that answered badly, and the
/// report has to say which of the two happened.
#[test]
fn origin_status_reports_a_control_origin_that_refuses_the_connection() {
    let fixture = Fixture::new();
    let upstream = Upstream::bind();
    let hostname = declared(&fixture, &upstream);
    let port = free_port();
    assert!(
        !accepts(port),
        "the case's premise is a closed port, and 127.0.0.1:{port} accepted"
    );

    let reported = status(&fixture, &format!("http://127.0.0.1:{port}"));
    assert_eq!(reported.status.code(), Some(1), "{}", stderr(&reported));
    let rows = report(&reported);
    let row = only_row(&rows);
    assert_not_public(row, &hostname);

    let edge = &row["edge_selection"];
    assert_eq!(
        edge["endpoint"],
        json!(format!("http://127.0.0.1:{port}{SELECTION_PATH}"))
    );
    assert_eq!(edge["state"], json!("unreadable"));
    let detail = edge["detail"]
        .as_str()
        .expect("the edge reading has a detail");
    assert!(
        detail.starts_with("the public edge did not answer:"),
        "a refused connection must be reported as no answer at all: {detail}"
    );
}

/// The publication reading, taken from this node's own handler table.
///
/// `converge` without `--apply` sends nothing: it reads what this machine
/// publishes and plans the difference. This machine does not publish its own
/// kernel name through a funnel, so the declared path is planned rather than
/// present and the receipt refuses. A machine carrying the tailscale CLI at
/// none of the paths the product's allowlist declares cannot answer the
/// question at all, and says so instead — the other real state of the same
/// read.
#[test]
fn converging_this_hosts_publication_plans_the_path_and_refuses_an_unpublished_origin() {
    let fixture = Fixture::new();
    let upstream = Upstream::bind();
    let hostname = declared(&fixture, &upstream);
    let planned = fixture.stado(&["web", "origin", "converge", ORIGIN, "--json"]);
    assert_eq!(
        planned.status.code(),
        Some(1),
        "an origin this host does not publish must exit non-zero: {}",
        stderr(&planned)
    );

    let Some(cli) = crate::listeners::tailscale_cli() else {
        let complaint = stderr(&planned);
        assert!(
            complaint.contains(
                "the tailscale CLI is installed at none of its approved paths on this host"
            ),
            "a host without the CLI must say the read could not be made: {complaint}"
        );
        return;
    };

    let receipt = report(&planned);
    assert_eq!(receipt["applied"], json!(false));
    assert_eq!(receipt["status"], json!("refused"));
    assert_eq!(receipt["target"], json!(TARGET));
    assert_eq!(
        receipt["origin"],
        json!(format!("https://{hostname}")),
        "the plan is about the origin the declaration spells"
    );
    assert_eq!(
        receipt["funnel"]["published_paths"],
        json!([]),
        "{cli} reported this host already publishing the declared path: {receipt}"
    );
    assert_eq!(receipt["funnel"]["missing_paths"], json!([PUBLISHED_PATH]));
    let handler = only_row(&receipt["handlers"]);
    assert_eq!(handler["path"], json!(PUBLISHED_PATH));
    assert_eq!(handler["change"], json!("add"));
    assert_eq!(
        handler["upstream"],
        json!(format!("{}{PUBLISHED_PATH}", upstream.origin())),
        "the planned handler forwards to the upstream this case bound"
    );
    assert_eq!(
        receipt["refusal"],
        json!(format!(
            "{TARGET} does not publish {PUBLISHED_PATH} as declared; funnel_enabled=false"
        ))
    );
    assert_eq!(
        receipt["readback"]["state"],
        json!("not-attempted"),
        "no public request is made to a name with no public address record"
    );
}
