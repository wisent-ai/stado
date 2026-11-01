//! An install moving the network node off the binary it replaces.
//!
//! A node unit written before the node had its own copy runs the program
//! every install replaces (`~/.local/bin/stado`, `~/.stado/bin/stado`), and
//! macOS keeps a Local Network decision per executable: after each install
//! the node, restarted onto the new file or still on the replaced image, was
//! refused every peer on the LAN with "No route to host" while a fresh
//! process reached them. So an install that replaced the node's program
//! moves the unit onto the node's own copy, placed from the installed file,
//! which later installs leave in place.

use std::path::Path;

use crate::targets::ComputeTarget;

/// What an install that placed the files at `replaced` does with this
/// machine's node unit: none on a machine whose node unit is absent or runs
/// another program (its own copy among them); otherwise the line saying the
/// unit now runs the node's own copy, or the line saying why it could not be
/// moved. The install goes on either way.
pub(crate) async fn move_off_replaced(
    target: &ComputeTarget,
    replaced: &[String],
) -> Option<Result<String, String>> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let (plan, home) = match super::plan(target) {
        Ok(planned) => planned,
        Err(error) => {
            return Some(Err(format!(
                "the network node's unit could not be planned, so whether it runs a program \
                 this install replaced is unknown: {error}; `stado network up` on this machine \
                 moves it onto the node's own copy"
            )))
        }
    };
    let unit = plan.unit_path(&home);
    let copy = super::pinned_program(&home);
    let declared = match crate::deploy::service::local_unit_file(
        &unit.to_string_lossy(),
        crate::deploy::service::KIND_LAUNCHD,
    ) {
        Ok(Some(declared)) => declared.program,
        Ok(None) => return None,
        Err(error) => {
            return Some(Err(format!(
                "the network node's unit {} could not be read, so whether it runs a program this \
                 install replaced is unknown: {error}; `stado network up` on this machine moves it \
                 onto the node's own copy {}",
                unit.display(),
                copy.display()
            )))
        }
    };
    let declared_file = std::fs::canonicalize(&declared).ok();
    let installed = replaced.iter().find(|path| {
        declared_file.is_some() && std::fs::canonicalize(path).ok() == declared_file
    })?;
    Some(
        match super::install_from(target, Path::new(installed)).await {
            Ok(_) => Ok(format!(
                "the network node's unit {} ran {declared}, which every install replaces; it now \
                 runs {}, which installs leave in place. macOS asks once more at this Mac whether \
                 “stado” may find devices on the local network; allowed, the node keeps it across \
                 installs",
                unit.display(),
                copy.display()
            )),
            Err(error) => Err(format!(
                "the network node's unit {} runs {declared}, which this install replaced, and could \
                 not be moved onto the node's own copy {}: {error}; `stado network up` on this \
                 machine moves it",
                unit.display(),
                copy.display()
            )),
        },
    )
}
