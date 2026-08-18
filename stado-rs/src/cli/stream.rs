//! `stado stream` — declare, provision and operate an interactive session on a
//! fleet host, and say where to point the client.
//!
//! The question this answers is "use that machine's GPU from this laptop". A
//! board cannot be borrowed over a network, so the fleet renders on the host and
//! the client receives frames. Declaring it in the registry keeps the fleet's
//! own rule: the placement fact lives where every other placement fact lives.

use clap::Subcommand;
use serde_json::Value;

use super::CmdError;
use crate::deploy::{host_channel, production_runner, stream as remote};
use crate::stream::schema::{
    DisplayStream, DEFAULT_LIBRARY_DIR, DEFAULT_REFRESH_HZ, DEFAULT_RESOLUTION,
    SUNSHINE_HTTPS_PORT,
};

fn click(error: impl ToString) -> CmdError {
    CmdError::click(error.to_string())
}

fn default_refresh() -> u16 {
    DEFAULT_REFRESH_HZ
}

#[derive(Subcommand, Debug)]
pub enum StreamCommands {
    /// Report what a host could render and encode, without changing it.
    Probe {
        target: String,
        #[arg(long)]
        json: bool,
    },
    /// Declare that this host carries an interactive session, in the registry.
    Declare {
        target: String,
        /// Virtual screen size the client receives, `WIDTHxHEIGHT`.
        #[arg(long, default_value = DEFAULT_RESOLUTION)]
        resolution: String,
        #[arg(long, default_value_t = default_refresh())]
        refresh_hz: u16,
        /// Driver UUID of the board that renders. Omitted leaves the driver's
        /// default, which is the board the job agent also prefers.
        #[arg(long)]
        gpu_uuid: Option<String>,
        /// Directory for large client data on a volume that has room.
        #[arg(long, default_value = DEFAULT_LIBRARY_DIR)]
        library_dir: String,
        /// Install Steam beside the session.
        #[arg(long)]
        steam: bool,
        /// Pin a Sunshine artifact explicitly, for a distribution this build has
        /// no measured digest for. Requires `--sunshine-sha256`.
        #[arg(long)]
        sunshine_url: Option<String>,
        /// sha256 of that artifact, measured, never guessed.
        #[arg(long)]
        sunshine_sha256: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Reconcile the host to its declaration: screen, session, Sunshine, units.
    Apply {
        target: String,
        /// Bind the declared library directory onto the host's largest
        /// disk-backed filesystem when it would otherwise land on a root volume
        /// with no room. Without this, such a host is refused and its mounts are
        /// named, because reshaping storage is not something to do quietly.
        #[arg(long)]
        provision_library: bool,
        #[arg(long)]
        json: bool,
    },
    /// What the session is doing right now, and where to point the client.
    Status {
        target: String,
        #[arg(long)]
        json: bool,
    },
    /// Hand Moonlight's four-digit PIN to Sunshine (no browser involved).
    Pair {
        target: String,
        #[arg(long)]
        pin: String,
        /// Name recorded for the paired client.
        #[arg(long, default_value = "moonlight")]
        client: String,
        #[arg(long)]
        json: bool,
    },
    /// Stop the session. `--purge` also removes the units and the screen.
    Stop {
        target: String,
        #[arg(long)]
        purge: bool,
        #[arg(long)]
        json: bool,
    },
}

pub async fn dispatch(command: StreamCommands) -> Result<(), CmdError> {
    match command {
        StreamCommands::Probe { target, json } => probe(&target, json).await,
        StreamCommands::Declare {
            target,
            resolution,
            refresh_hz,
            gpu_uuid,
            library_dir,
            steam,
            sunshine_url,
            sunshine_sha256,
            json,
        } => {
            declare(
                &target,
                &resolution,
                refresh_hz,
                gpu_uuid,
                &library_dir,
                steam,
                sunshine_url,
                sunshine_sha256,
                json,
            )
            .await
        }
        StreamCommands::Apply {
            target,
            provision_library,
            json,
        } => apply(&target, provision_library, json).await,
        StreamCommands::Status { target, json } => status(&target, json).await,
        StreamCommands::Pair {
            target,
            pin,
            client,
            json,
        } => pair(&target, &pin, &client, json).await,
        StreamCommands::Stop {
            target,
            purge,
            json,
        } => stop(&target, purge, json).await,
    }
}

fn field(report: &Value, name: &str) -> String {
    report
        .get("fields")
        .and_then(|fields| fields.get(name))
        .map(|value| match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn emitted(report: &Value, json: bool, expected: &str) -> Result<bool, CmdError> {
    if report.get("status").and_then(Value::as_str) != Some(expected) {
        return Err(CmdError::click(format!(
            "stream operation did not reach {expected}: {report}"
        )));
    }
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(false);
    }
    Ok(true)
}

async fn probe(target_name: &str, json: bool) -> Result<(), CmdError> {
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

async fn declare(
    target_name: &str,
    resolution: &str,
    refresh_hz: u16,
    gpu_uuid: Option<String>,
    library_dir: &str,
    steam: bool,
    sunshine_url: Option<String>,
    sunshine_sha256: Option<String>,
    json: bool,
) -> Result<(), CmdError> {
    // The artifact that installs is a property of the host's distribution, so
    // the host is asked before anything is written down.
    let target = host_channel::canonical_target(target_name)
        .await
        .map_err(click)?;
    let probed = remote::probe(&target, &production_runner())
        .await
        .map_err(click)?;
    let release = field(&probed, "release");
    let mut declaration =
        remote::default_declaration(resolution, refresh_hz, gpu_uuid, library_dir, steam, &release)
            .map_err(CmdError::click)?;
    match (sunshine_url, sunshine_sha256) {
        (Some(url), Some(digest)) => {
            declaration.sunshine.deb_url = url;
            declaration.sunshine.deb_sha256 = digest;
        }
        (None, None) => {}
        _ => {
            return Err(CmdError::click(
                "--sunshine-url and --sunshine-sha256 go together: an artifact without a measured \
                 digest is not pinned",
            ))
        }
    }
    declaration
        .validate(&format!("targets[{target_name}].display_stream"))
        .map_err(CmdError::click)?;

    let mut document = super::registry::fetch_document().await?;
    let targets = document
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| CmdError::click("registry carries no targets array"))?;
    let entry = targets
        .iter_mut()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(target_name))
        .ok_or_else(|| CmdError::click(format!("registry has no target named {target_name:?}")))?;
    let object = entry
        .as_object_mut()
        .ok_or_else(|| CmdError::click("registry target is not an object"))?;
    object.insert(
        "display_stream".to_string(),
        serde_json::to_value(&declaration)?,
    );
    crate::targets::load_registry_from_str(&serde_json::to_string(&document)?)
        .map_err(|error| CmdError::click(format!("the edited registry does not load: {error}")))?;
    let version = super::registry::push_document(&document).await?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "target": target_name,
                "declaration": declaration,
                "store_version": version,
            }))?
        );
        return Ok(());
    }
    println!("{target_name}: declared an interactive session");
    println!("  release:  {release}");
    println!("  screen:   {resolution} at {refresh_hz} Hz");
    println!(
        "  board:    {}",
        declaration
            .gpu_uuid
            .clone()
            .unwrap_or_else(|| "driver default".to_string())
    );
    println!("  library:  {}", declaration.library_dir);
    println!("  sunshine: {}", declaration.sunshine.version);
    println!("  steam:    {}", declaration.steam);
    println!("apply it with `stado stream apply {target_name}`");
    Ok(())
}

fn declaration_of(target: &crate::targets::ComputeTarget) -> Result<DisplayStream, CmdError> {
    target.display_stream.clone().ok_or_else(|| {
        CmdError::click(format!(
            "{} declares no interactive session; run `stado stream declare {}` first",
            target.name, target.name
        ))
    })
}

async fn apply(
    target_name: &str,
    provision_library: bool,
    json: bool,
) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(target_name)
        .await
        .map_err(click)?;
    let declaration = declaration_of(&target)?;
    if !declaration.enabled {
        return Err(CmdError::click(format!(
            "{target_name} declares display_stream.enabled = false; nothing to apply"
        )));
    }
    let runner = production_runner();
    // The declaration names a board by driver UUID; Xorg addresses one by PCI
    // bus id, and only the host knows the mapping.
    let probed = remote::probe(&target, &runner).await.map_err(click)?;
    let bus_id = remote::bus_id_for(&probed, declaration.gpu_uuid.as_deref()).ok_or_else(|| {
        CmdError::click(match &declaration.gpu_uuid {
            Some(uuid) => format!("{target_name} reports no board with uuid {uuid}"),
            None => format!("{target_name} reports no NVIDIA board at all"),
        })
    })?;
    let report = remote::install(&target, &declaration, &bus_id, provision_library, &runner)
        .await
        .map_err(click)?;
    let report = remote::with_declaration(report, &declaration);
    if !emitted(&report, json, "installed")? {
        return Ok(());
    }
    println!("{target_name}: session provisioned on {bus_id}");
    println!("  packages: {}", field(&report, "packages"));
    println!("  sunshine: {}", field(&report, "sunshine"));
    println!("  screen:   {}", field(&report, "session"));
    println!(
        "  units:    xorg {}, sunshine {}",
        field(&report, "xorg"),
        field(&report, "sunshine_state")
    );
    println!("  ports:    {}", field(&report, "ports"));
    println!("pair a client with `stado stream pair {target_name} --pin XXXX`");
    Ok(())
}

async fn status(target_name: &str, json: bool) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(target_name)
        .await
        .map_err(click)?;
    let declaration = target.display_stream.clone();
    let report = remote::status(&target, &production_runner())
        .await
        .map_err(click)?;
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

async fn pair(
    target_name: &str,
    pin: &str,
    client: &str,
    json: bool,
) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(target_name)
        .await
        .map_err(click)?;
    let report = remote::pair(&target, pin, client, &production_runner())
        .await
        .map_err(click)?;
    if !emitted(&report, json, "paired")? {
        return Ok(());
    }
    println!("{target_name}: paired {client} (HTTP {})", field(&report, "http"));
    Ok(())
}

async fn stop(target_name: &str, purge: bool, json: bool) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(target_name)
        .await
        .map_err(click)?;
    let report = remote::stop(&target, purge, &production_runner())
        .await
        .map_err(click)?;
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
