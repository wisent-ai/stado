//! What a host could render and encode, read without changing it.

use serde_json::Value;

use crate::cli::stream::report::{emitted, field};
use crate::cli::CmdError;
use crate::deploy::{production_runner, stream as remote};

pub(in crate::cli::stream) async fn probe(target_name: &str, json: bool) -> Result<(), CmdError> {
    let target = crate::cli::canonical_host(target_name).await?;
    let report = remote::probe(&target, &production_runner())
        .await
        .map_err(CmdError::from)?;
    if !emitted(&report, json, "probed")? {
        return Ok(());
    }
    println!("host:      {} ({})", target_name, field(&report, "host"));
    println!("driver:    {}", field(&report, "driver"));
    // The host lists one board as text or several as a list; a report that
    // names none, or names them in another shape, says so.
    match report.get("fields").and_then(|fields| fields.get("board")) {
        Some(Value::String(single)) => println!("board:     {single}"),
        Some(Value::Array(list)) => {
            // A board entry that is not text is printed as the host wrote
            // it, not as an empty name.
            for board in list {
                match board.as_str() {
                    Some(name) => println!("board:     {name}"),
                    None => println!("board:     {board} (not a board name)"),
                }
            }
        }
        Some(other) => println!("board:     {other} (not a board name)"),
        None => println!("board:     none reported"),
    }
    println!("drm:       {}", field(&report, "drm_nodes"));
    println!("session:   Xorg {}", field(&report, "xorg_installed"));
    println!("sunshine:  {}", field(&report, "sunshine_installed"));
    println!("dm:        {}", field(&report, "display_manager"));
    println!("client at: {}", field(&report, "tailscale"));
    println!(
        "space:     root {} KiB free, library {} KiB free",
        field(&report, "root_free_kib"),
        field(&report, "library_free_kib")
    );
    println!("units:     {}", field(&report, "units"));
    Ok(())
}
