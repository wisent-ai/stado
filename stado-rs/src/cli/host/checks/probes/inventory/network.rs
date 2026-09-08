use serde_json::Value;

use crate::cli::host::checks::probes::cell;

/// The forward-marker, listener and subcommand tables of
/// [`super::inventory`].
pub(super) fn print_network(report: &Value, section: &dyn Fn(&str) -> Vec<Value>, target: &str) {
    let markers = section("forwards");
    if markers.is_empty() {
        println!(
            "\nforward markers: none ($HOME/.stado/forwards is {})",
            cell(report.get("forwards_dir_state"))
        );
    } else {
        // Two verdict columns because the marker is reconciled against two
        // independent things: LISTENING is whether anything answers where
        // the marker points, DECLARATION is whether the registry sends
        // consumers to the same place. A marker can pass one and fail the
        // other, and collapsing them would hide exactly that case.
        crate::cli::table::print(
            &[
                "MARKER",
                "STATE",
                "URL",
                "PORT",
                "LISTENING",
                "DECLARED URL",
                "DECLARATION",
            ],
            &markers
                .iter()
                .map(|marker| {
                    vec![
                        cell(marker.get("name")),
                        cell(marker.get("state")),
                        cell(marker.get("url")),
                        cell(marker.get("port")),
                        cell(marker.get("reconciliation")),
                        cell(marker.get("declared_url")),
                        cell(marker.get("declaration_verdict")),
                    ]
                })
                .collect::<Vec<Vec<String>>>(),
        );
    }

    let listeners = section("listeners");
    crate::cli::table::print(
        &["PORT", "PID", "ADDRESS"],
        &listeners
            .iter()
            .map(|listener| {
                vec![
                    cell(listener.get("port")),
                    cell(listener.get("pid")),
                    cell(listener.get("address")),
                ]
            })
            .collect::<Vec<Vec<String>>>(),
    );
    let listeners_state = report.get("listeners_state");
    if listeners_state.and_then(Value::as_str)
        != Some(crate::deploy::host_inventory::LISTENERS_READ)
    {
        // An empty table above and this line missing would read as "nothing
        // is listening on this host", which is the opposite of what happened.
        println!(
            "listeners: {} — the kernel socket table could not be read, so no \
             marker above could be checked against it",
            cell(listeners_state)
        );
    }
    if !listeners.is_empty() {
        // Say what the pid column is NOT, once, where it is being read.
        println!(
            "Owners are pids. Map one to a program with stado host exec {target} \
             -- ps ax -o pid -o ppid -o etime -o comm; this command never reads \
             process arguments or environments."
        );
    }

    crate::cli::table::print(
        &["SUBCOMMAND", "INSTALLED BINARY"],
        &section("subcommands")
            .iter()
            .map(|subcommand| vec![cell(subcommand.get("name")), cell(subcommand.get("state"))])
            .collect::<Vec<Vec<String>>>(),
    );
}
