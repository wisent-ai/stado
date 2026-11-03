//! Where a service surface built from source is placed: its whole staged
//! tree, under the release tree the catalog program of its service runs from.
//!
//! A source build of a service was placed as a CLI is, each `bin/` member into
//! the shared `~/.stado/bin`, so Weles' `bin/start` launcher collided with the
//! file of the same name Skarbiec and Brama place there (`~/.stado/bin/start
//! belongs to another recorded product`), and the catalog program
//! (`$HOME/.stado/services/<service>/current/<platform>/<launcher>`) was never
//! written by the install at all. A service tree is placed where a delivered
//! release of it lands: `$HOME/.stado/services/<service>/current/<platform>`,
//! every staged member at its staged path, with no link on PATH.

use crate::catalog::text;
use crate::install::plan::Placement;
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::Path;

/// The directory the catalog program of `product`'s service names for this
/// machine: `$HOME/.stado/services/<directory service>/current/<platform>`,
/// read from the catalog's `service.program`.
fn tree_root(product: &Value, home: &Path) -> Result<std::path::PathBuf> {
    let program = text(&product["service"], "program")
        .context("a service installation needs the catalog's service.program")?;
    let platform = crate::common::platform()?;
    let short = platform.replace("arm64", "arm").replace("amd64", "amd");
    let resolved = program
        .replace("$HOME", &home.to_string_lossy())
        .replace("$STADO_PLATFORM", &short);
    let path = Path::new(&resolved);
    let root = path.parent().with_context(|| {
        format!("the catalog program {program} names no directory to install the service tree in")
    })?;
    let services = home.join(".stado/services");
    if !root.starts_with(&services) {
        bail!(
            "the catalog program {program} runs from {}, outside {}, where a service tree is \
             installed; nothing was installed",
            root.display(),
            services.display()
        );
    }
    Ok(root.to_path_buf())
}

/// Every staged member of `spec` placed under the service tree root, at its
/// staged path; refused when the catalog program itself is not among them.
pub(super) fn placements(
    product: &Value,
    spec: &Value,
    output: &Path,
    home: &Path,
) -> Result<Vec<Placement>> {
    let root = tree_root(product, home)?;
    let program = text(&product["service"], "program")?;
    let launcher = Path::new(program)
        .file_name()
        .context("the catalog program names no file")?
        .to_string_lossy()
        .into_owned();
    let mut placements = Vec::new();
    let mut carries_launcher = false;
    for (source_name, member) in spec["stage"]
        .as_object()
        .context("release platform has no stage map")?
    {
        let member = member.as_str().context("stage member must be a path")?;
        let relative = crate::common::relative(Path::new(member))?;
        carries_launcher |= relative == Path::new(&launcher);
        let source = super::manifest::inside(output, source_name)?;
        placements.push(Placement {
            source,
            destination: root.join(relative),
            symbolic: false,
        });
    }
    if !carries_launcher {
        bail!(
            "the release stages no {launcher}, the program the catalog runs the service as \
             ({program}); nothing was installed"
        );
    }
    Ok(placements)
}
