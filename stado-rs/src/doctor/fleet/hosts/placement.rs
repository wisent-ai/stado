//! Whether this host is holding a port belonging to a service the directory
//! places somewhere else.

use crate::doctor::Check;

pub(in crate::doctor) const PLACEMENT_ID: &str = "placement";
pub(in crate::doctor) const PLACEMENT_TITLE: &str = "Service placement";
pub(in crate::doctor) const PLACEMENT_REMEDY: &str =
    "`stado service stop NAME --host HOST` ends an instance nothing placed here";

/// Run a probe under [`PROBE_TIMEOUT`]. An elapsed probe becomes a FAIL
/// row rather than a hung command, so the remaining probes still report.
/// Nothing serving here that is placed somewhere else.
///
/// A gateway is placed on exactly one host. A second copy listening on the same
/// port on another machine does not announce itself: callers that resolve a
/// loopback address reach it, it authenticates against its own stale view, and
/// the refusal reads as a credential fault. Cheap to detect from here, because
/// the only thing to look at is whether this host is holding the port of a
/// service the directory places elsewhere.
pub(in crate::doctor) async fn check_placement() -> Check {
    let document = match crate::cli::registry::fetch_document().await {
        Ok(document) => document,
        Err(error) => {
            // The registry is this check's only source of truth, so a
            // registry nobody could read leaves the question unanswered
            // rather than answered badly. Same distinction as an elapsed
            // probe: no reading, therefore no verdict.
            return Check::unmeasured(
                PLACEMENT_ID,
                PLACEMENT_TITLE,
                format!("not measured: the registry could not be read: {error}"),
                PLACEMENT_REMEDY,
            );
        }
    };
    let here = crate::providers::vast::system_hostname();
    let services = document
        .get("service_directory")
        .and_then(|block| block.get("services"))
        .and_then(serde_json::Value::as_object);
    let Some(services) = services else {
        return Check::pass(
            PLACEMENT_ID,
            PLACEMENT_TITLE,
            "the directory declares no services".to_string(),
            PLACEMENT_REMEDY,
        )
        .measuring(PLACEMENT_ID, Some(here), 0);
    };
    let mut squatting: Vec<String> = Vec::new();
    let mut absent: Vec<String> = Vec::new();
    // Declared services whose port this host actually probed. A directory
    // entry with no active host or no resolvable port is not a subject: it
    // was skipped, and counting it would inflate the only number that says
    // whether this row means anything.
    let mut probed = 0_u64;
    for (name, entry) in services {
        let Some(active) = entry.get("active_host").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let Some(port) = crate::cli::directory::service_port(entry, active) else {
            continue;
        };
        probed += 1;
        if active.starts_with(&here) || here.starts_with(active) {
            // The placed host itself: the question is the opposite one. A
            // directory entry naming this host while nothing answers its port
            // is a service the fleet believes in and cannot reach, which is
            // how a gateway sat restarting for hours with every row here
            // green.
            if tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_err()
            {
                absent.push(format!(
                    "{name} is placed here and nothing answers port {port}"
                ));
            }
            continue;
        }
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            squatting.push(format!(
                "{name} is placed on {active}, and port {port} answers here"
            ));
        }
    }
    let problems: Vec<String> = squatting.into_iter().chain(absent).collect();
    let verdict = if problems.is_empty() {
        Check::pass(
            PLACEMENT_ID,
            PLACEMENT_TITLE,
            format!(
                "{probed} declared service port(s) probed from here; this host holds no port \
                 belonging to a service placed elsewhere, and every service placed here answers"
            ),
            PLACEMENT_REMEDY,
        )
    } else {
        Check::fail(
            PLACEMENT_ID,
            PLACEMENT_TITLE,
            problems.join("; "),
            PLACEMENT_REMEDY,
        )
    };
    verdict.measuring(PLACEMENT_ID, Some(here), probed)
}
