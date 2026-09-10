use serde_json::Value;

use crate::cli::host::checks::probes::cell;

/// The managed-binary and Cargo-path tables of [`super::inventory`].
pub(super) fn print_binaries(report: &Value, section: &dyn Fn(&str) -> Vec<Value>) {
    crate::cli::reporting::table::print(
        &[
            "BINARY",
            "STATE",
            "EXECUTABLE",
            "VERSION STATE",
            "VERSION",
            "DECLARED",
            "VERDICT",
        ],
        &section("managed_binaries")
            .iter()
            .map(|binary| {
                vec![
                    cell(binary.get("name")),
                    cell(binary.get("state")),
                    cell(binary.get("executable")),
                    cell(binary.get("version_state")),
                    cell(binary.get("version")),
                    cell(binary.get("declared_version")),
                    cell(binary.get("version_verdict")),
                ]
            })
            .collect::<Vec<Vec<String>>>(),
    );

    // The fixed Cargo paths and their children are part of the same typed
    // report in JSON and text. Keeping the table here avoids a second
    // filesystem reader that could drift from the JSON document.
    let cargo = report.get("cargo").and_then(Value::as_object);
    let cargo_roots = [("$HOME/.cargo", "home"), ("$HOME/.cargo/bin", "bin")]
        .iter()
        .filter_map(|(path, key)| {
            cargo.and_then(|value| value.get(*key)).map(|metadata| {
                vec![
                    (*path).to_string(),
                    cell(metadata.get("kind")),
                    cell(metadata.get("metadata_state")),
                    cell(metadata.get("mode")),
                    cell(metadata.get("uid")),
                    cell(metadata.get("gid")),
                    cell(metadata.get("bytes")),
                    cell(metadata.get("modified_epoch")),
                    cell(metadata.get("symlink_target")),
                    cell(metadata.get("symlink_target_state")),
                ]
            })
        })
        .collect::<Vec<Vec<String>>>();
    crate::cli::reporting::table::print(
        &[
            "CARGO PATH",
            "TYPE",
            "METADATA",
            "MODE",
            "UID",
            "GID",
            "BYTES",
            "MODIFIED",
            "LINK TARGET",
            "LINK STATE",
        ],
        &cargo_roots,
    );
    let cargo_entries = cargo
        .and_then(|value| value.get("entries"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    crate::cli::reporting::table::print(
        &[
            "CARGO BIN ENTRY",
            "NAME STATE",
            "TYPE",
            "METADATA",
            "MODE",
            "UID",
            "GID",
            "BYTES",
            "MODIFIED",
            "LINK TARGET",
            "LINK STATE",
        ],
        &cargo_entries
            .iter()
            .map(|metadata| {
                vec![
                    cell(metadata.get("name")),
                    cell(metadata.get("name_state")),
                    cell(metadata.get("kind")),
                    cell(metadata.get("metadata_state")),
                    cell(metadata.get("mode")),
                    cell(metadata.get("uid")),
                    cell(metadata.get("gid")),
                    cell(metadata.get("bytes")),
                    cell(metadata.get("modified_epoch")),
                    cell(metadata.get("symlink_target")),
                    cell(metadata.get("symlink_target_state")),
                ]
            })
            .collect::<Vec<Vec<String>>>(),
    );
    println!(
        "Cargo bin membership: state={} listed={} seen={} entries_complete={} complete={}",
        cell(cargo.and_then(|value| value.get("entries_state"))),
        cargo_entries.len(),
        cell(cargo.and_then(|value| value.get("entries_seen"))),
        cell(cargo.and_then(|value| value.get("entries_complete"))),
        cell(cargo.and_then(|value| value.get("complete"))),
    );
}
