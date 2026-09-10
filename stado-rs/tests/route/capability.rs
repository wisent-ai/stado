//! Resolving a declared capability route on this machine, through the real
//! broker installed on it.
//!
//! `stado route capability` resolves the service's active host, and the host
//! is this one, so the production channel runs the real `skarbiec` at
//! `$HOME/.stado/bin/skarbiec` against the real GnuPG vault beside it. The
//! vault path in the report comes back from that execution: an answer naming
//! the tempdir could not have come from anywhere else.

use serde_json::{json, Value};

use super::broker::{self, Vault, FIELD, ITEM, RESOURCE};
use super::fleet::{json_stdout, said, Fleet, SERVICE, TARGET};

/// One fleet with the service declared straight into the directory, so the
/// capability cases exercise resolution rather than re-proving declaration.
fn fleet_with_service() -> Fleet {
    let fleet = Fleet::new();
    let mut document = fleet.registry();
    document["service_directory"]["services"] = json!({
        SERVICE: {
            "active_host": TARGET,
            "endpoints": {TARGET: {"url": format!("http://127.0.0.1:{}", super::fleet::PORT)}},
            "consumers": {},
        }
    });
    fleet.write_registry(&document);
    fleet
}

#[test]
fn a_declared_route_resolves_through_the_real_broker_to_the_coordinate_it_names() {
    let fleet = fleet_with_service();
    let vault = Vault::install(&fleet, &broker::current());

    let output = fleet.stado_with(
        &["route", "capability", SERVICE, "--json"],
        &[("GNUPGHOME", vault.gnupg_home())],
    );
    assert!(
        output.status.success(),
        "resolving a declared route failed:\n{}",
        said(&output)
    );
    let report = json_stdout(&output);
    assert_eq!(report["service"], SERVICE);
    assert_eq!(report["active_host"], TARGET);
    assert_eq!(
        report["vault"].as_str().map(std::path::Path::new),
        Some(vault.vault_file()),
        "the answer did not come from the isolated host's own vault"
    );

    let rows = report["report"]["routes"]
        .as_array()
        .unwrap_or_else(|| panic!("no route rows in {}", said(&output)));
    let resolved = rows
        .iter()
        .find(|row| row["resource"] == RESOURCE)
        .unwrap_or_else(|| panic!("{RESOURCE} is absent from {}", said(&output)));
    assert_eq!(resolved["item"], ITEM);
    assert_eq!(resolved["field"], FIELD);
    assert_eq!(
        (&resolved["item_present"], &resolved["field_present"]),
        (&Value::Bool(true), &Value::Bool(true)),
        "the broker could not open the item this route names: {resolved}"
    );

    // The state behind the answer, read off the host rather than off stdout:
    // the broker's own table maps the same resource to the same coordinate.
    let table: Value = serde_json::from_str(&std::fs::read_to_string(&vault.table).unwrap())
        .expect("the persisted capability route table is JSON");
    assert_eq!(table[RESOURCE]["item"], ITEM);
    assert_eq!(table[RESOURCE]["field"], FIELD);
}
