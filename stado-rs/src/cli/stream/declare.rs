//! Writing the declaration down: ask the host what artifact its distribution
//! takes, then put the decided declaration on the target's registry entry.

use serde_json::Value;

use super::report::{click, field};
use crate::cli::registry::commit_document;
use crate::cli::CmdError;
use crate::deploy::{host_channel, production_runner, stream as remote};

// Nine parameters mirror the `stado stream declare` CLI surface one-to-one;
// bundling them into a struct would only rename the same nine flags.
#[allow(clippy::too_many_arguments)]
pub(super) async fn declare(
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
    let mut declaration = remote::default_declaration(
        resolution,
        refresh_hz,
        gpu_uuid,
        library_dir,
        steam,
        &release,
    )
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

    // Pure: the declaration was decided by probing the host above, and
    // writing it onto the target entry is a function of whatever document is
    // current. A lost race re-applies it to the newer document rather than
    // republishing this one over the winner's edit.
    let version = commit_document(|current| {
        let mut document = current.clone();
        let targets = document
            .get_mut("targets")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| CmdError::click("registry carries no targets array"))?;
        let entry = targets
            .iter_mut()
            .find(|entry| entry.get("name").and_then(Value::as_str) == Some(target_name))
            .ok_or_else(|| {
                CmdError::click(format!("registry has no target named {target_name:?}"))
            })?;
        let object = entry
            .as_object_mut()
            .ok_or_else(|| CmdError::click("registry target is not an object"))?;
        object.insert(
            "display_stream".to_string(),
            serde_json::to_value(&declaration)?,
        );
        crate::targets::load_registry_from_str(&serde_json::to_string(&document)?).map_err(
            |error| CmdError::click(format!("the edited registry does not load: {error}")),
        )?;
        Ok(document)
    })
    .await?;

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
