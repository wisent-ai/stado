//! What a host could render and encode, read without changing it.

use serde_json::Value;

use crate::cli::stream::report::{click, emitted, field};
use crate::cli::CmdError;
use crate::deploy::{host_channel, production_runner, stream as remote};

pub(in crate::cli::stream) async fn probe(target_name: &str, json: bool) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(target_name)
        .await
        .map_err(click)?;
    let report = remote::probe(&target, &production_runner())
        .await
        .map_err(click)?;
    if !emitted(&report, json, "probed")? {
        return Ok(());
    }
    println!("host:      {} ({})", target_name, field(&report, "host"));
    println!("driver:    {}", field(&report, "driver"));
    let boards = report
        .get("fields")
        .and_then(|fields| fields.get("board"))
        .cloned()
        .unwrap_or(Value::Null);
    match &boards {
        Value::String(single) => println!("board:     {single}"),
        Value::Array(list) => {
            for board in list {
                println!("board:     {}", board.as_str().unwrap_or_default());
            }
        }
        _ => println!("board:     none reported"),
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
