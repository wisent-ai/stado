//! The directory a static site is served from.

use std::path::{Path, PathBuf};

use crate::cli::CmdError;

/// The directory the static site is served from.
///
/// **The rule: the site root is what `--root` names, and the checkout root
/// when nothing names one.** There is deliberately no search of `public`,
/// `site`, `dist` and `.` in some order: a probe order is a guess, and a build
/// that guesses where the product's files are stages a directory nobody
/// declared. `tama-landing` writes into `dist/` and `jeden`'s site is `web/`,
/// so both name it; the four sites that are `index.html` at the top of the
/// repository name nothing.
///
/// The path is repository-relative and may not climb out of the checkout,
/// which is the same rule `.wisent-release.json`'s own staged paths are held
/// to.
pub(in crate::cli::web::builds) fn site_root(
    source: &Path,
    declared: Option<&str>,
) -> Result<PathBuf, CmdError> {
    let Some(declared) = declared else {
        return Ok(source.to_path_buf());
    };
    if !crate::release_pipeline::safe_relative(declared) {
        return Err(CmdError::click(format!(
            "--root {declared} is not a repository-relative path: a site root is a directory inside the checkout"
        )));
    }
    let root = source.join(declared);
    if !root.is_dir() {
        return Err(CmdError::click(format!(
            "--root names {}, which is not a directory: the recipe declares the site root, and after the build there is nothing there to serve",
            root.display()
        )));
    }
    Ok(root)
}
