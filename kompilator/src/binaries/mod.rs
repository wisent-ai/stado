//! Where a build writes the package's binaries, and which binaries it links.
//!
//! The directory is decided here once: `build` writes there and prints each
//! binary it linked, and `where` answers the same paths without building, so
//! a caller never has to spell `.build/kompilator/<profile>` itself and a
//! binary that was never built is refused with the command that builds it.

use std::path::{Path, PathBuf};

use crate::manifest::{Bin, Manifest};

/// The directory a build of `checkout_dir` writes its binaries to: `out`
/// when named, else `<checkout_dir>/.build/kompilator/<release or debug>`.
pub fn out_dir(checkout_dir: &Path, release: bool, out: Option<&Path>) -> PathBuf {
    match out {
        Some(out) => out.to_path_buf(),
        None => checkout_dir
            .join(".build/kompilator")
            .join(if release { "release" } else { "debug" }),
    }
}

/// The binaries of `manifest` that `named` selects: every one when none is
/// named. A name the package does not declare is refused with the names it
/// does declare.
pub fn wanted<'a>(manifest: &'a Manifest, named: &[String]) -> Result<Vec<&'a Bin>, String> {
    if let Some(unknown) = named.iter().find(|name| !manifest.bins.iter().any(|bin| &bin.name == *name)) {
        let declared: Vec<&str> = manifest.bins.iter().map(|bin| bin.name.as_str()).collect();
        return Err(format!(
            "--bin {unknown}: {} {} declares no such binary; it declares {}",
            manifest.name,
            manifest.version,
            match declared.is_empty() {
                true => String::from("none"),
                false => declared.join(", "),
            }
        ));
    }
    let selected: Vec<&Bin> = manifest
        .bins
        .iter()
        .filter(|bin| named.is_empty() || named.contains(&bin.name))
        .collect();
    if selected.is_empty() {
        return Err(format!("{} {} declares no binary to build", manifest.name, manifest.version));
    }
    Ok(selected)
}

/// The path of each selected binary in `out`, refusing one that is not
/// there with the `kompilator build` command that writes it.
pub fn located(bins: &[&Bin], out: &Path, build_command: &str) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for bin in bins {
        let path = out.join(&bin.name);
        match path.try_exists() {
            Ok(true) => paths.push(path),
            Ok(false) => {
                return Err(format!(
                    "{} has not been built: {} does not exist; `{build_command}` builds it there",
                    bin.name,
                    path.display()
                ))
            }
            Err(error) => return Err(format!("{} cannot be examined: {error}", path.display())),
        }
    }
    Ok(paths)
}
