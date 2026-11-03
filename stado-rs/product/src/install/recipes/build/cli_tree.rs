//! Where a CLI surface built from source is placed: each `bin/` member under
//! `~/.stado/bin` with a link on PATH, a nested launcher helper at its staged
//! path, and the product's share directory.

use crate::install::plan::Placement;
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::Path;

/// The placements of a CLI surface's staged members; refused when the
/// release stages no binary under `bin/`.
pub(super) fn placements(
    id: &str,
    document: &Value,
    spec: &Value,
    output: &Path,
    home: &Path,
) -> Result<Vec<Placement>> {
    let runtime_binary = document["runtime"]["binary"].as_str().unwrap_or(id);
    let mut placements = Vec::new();
    let mut binaries = 0;
    for (source_name, member) in spec["stage"]
        .as_object()
        .context("release platform has no stage map")?
    {
        let member = member.as_str().context("stage member must be a path")?;
        let member_path = crate::common::relative(Path::new(member))?;
        let root_binary = member == runtime_binary && member_path.components().count() == 1;
        // A file nested below bin/ (a launcher's sourced stages) is part of the
        // stage a bin/ entry runs from: it is installed at the same relative
        // path so that entry finds it, but it is no command of its own and
        // gets no link on PATH.
        let nested_helper = member.starts_with("bin/") && member_path.components().count() > 2;
        let binary = (member.starts_with("bin/") && !nested_helper) || root_binary;
        // A product's share directory is staged either as its members
        // (`share/<id>/…`) or as the one directory `share/<id>`; the release
        // archive places both, so a source install places both too.
        let shared = format!("share/{id}");
        let share_member = member == shared || member.starts_with(&format!("{shared}/"));
        if !binary && !nested_helper && !share_member {
            continue;
        }
        let source = super::manifest::inside(output, source_name).with_context(|| {
            format!(
                "{id} stage key '{source_name}' is relative to output {}",
                output.display()
            )
        })?;
        let destination = home.join(".stado").join(if root_binary {
            format!("bin/{member}")
        } else {
            member.to_owned()
        });
        // A directory directly under bin/ is a resource the commands beside it
        // read (a SwiftPM `<Package>_<Target>.bundle`, which Bundle.module finds
        // next to the executable): it is placed at the same path, whole, and
        // gets no link on PATH.
        let resource_directory = binary && !root_binary && source.is_dir();
        if (binary || nested_helper) && !resource_directory && !source.is_file() {
            bail!("CLI stage member must be a regular file under bin/, or a resource directory beside one: {member}");
        }
        if binary && !resource_directory {
            placements.push(Placement {
                source: destination.clone(),
                destination: home
                    .join(".local/bin")
                    .join(destination.file_name().unwrap()),
                symbolic: true,
            });
            binaries += 1;
        }
        placements.push(Placement {
            source,
            destination,
            symbolic: false,
        });
    }
    if binaries == 0 {
        bail!("{id} stages no CLI binary into bin/; nothing was installed");
    }
    Ok(placements)
}
