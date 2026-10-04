//! The session once it exists: what it is doing, which client may reach it,
//! and how it is put down.

use crate::cli::stream::report::{emitted, field};
use crate::cli::CmdError;
use crate::deploy::{production_runner, stream as remote};
use crate::stream::schema::SUNSHINE_HTTPS_PORT;

pub(in crate::cli::stream) async fn status(target_name: &str, json: bool) -> Result<(), CmdError> {
    let target = crate::cli::canonical_host(target_name).await?;
    let declaration = target.display_stream.clone();
    let report = remote::status(&target, &production_runner())
        .await
        .map_err(CmdError::from)?;
    let report = match &declaration {
        Some(value) => remote::with_declaration(report, value),
        None => report,
    };
    if !emitted(&report, json, "reported")? {
        return Ok(());
    }
    match &declaration {
        Some(value) => println!(
            "declared:  {} at {} Hz on {}",
            value.resolution,
            value.refresh_hz,
            value
                .gpu_uuid
                .clone()
                .unwrap_or_else(|| "driver default".to_string())
        ),
        None => println!("declared:  nothing — this host is headless by declaration"),
    }
    println!(
        "units:     xorg {}, sunshine {}",
        field(&report, "xorg"),
        field(&report, "sunshine")
    );
    println!("screen:    {}", field(&report, "session"));
    println!("rendering: {}", field(&report, "rendering_board"));
    println!("ports:     {}", field(&report, "ports"));
    println!("paired:    {} client(s)", field(&report, "paired_clients"));
    println!("library:   {}", field(&report, "library"));
    for (label, key) in [("xorg log", "xorg_log"), ("sunshine log", "sunshine_log")] {
        let line = field(&report, key);
        if !line.trim().is_empty() && line != "unknown" {
            println!("{label}:  {line}");
        }
    }
    let endpoint = field(&report, "client_endpoint");
    println!("client:    point Moonlight at {endpoint}:{SUNSHINE_HTTPS_PORT}");
    Ok(())
}

pub(in crate::cli::stream) async fn pair(
    target_name: &str,
    pin: &str,
    client: &str,
    json: bool,
) -> Result<(), CmdError> {
    let target = crate::cli::canonical_host(target_name).await?;
    let report = remote::pair(&target, pin, client, &production_runner())
        .await
        .map_err(CmdError::from)?;
    if !emitted(&report, json, "paired")? {
        return Ok(());
    }
    println!(
        "{target_name}: paired {client} (HTTP {})",
        field(&report, "http")
    );
    Ok(())
}

pub(in crate::cli::stream) async fn stop(
    target_name: &str,
    purge: bool,
    json: bool,
) -> Result<(), CmdError> {
    let target = crate::cli::canonical_host(target_name).await?;
    let report = remote::stop(&target, purge, &production_runner())
        .await
        .map_err(CmdError::from)?;
    if !emitted(&report, json, "stopped")? {
        return Ok(());
    }
    println!(
        "{target_name}: xorg {}, sunshine {}",
        field(&report, "xorg"),
        field(&report, "sunshine")
    );
    if purge {
        println!("  {}", field(&report, "purged"));
    } else {
        println!("  {}", field(&report, "kept"));
    }
    Ok(())
}
