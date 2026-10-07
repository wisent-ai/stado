//! Where the staged archive is: the path a coordinate names, and the one
//! expansion this performs on a root written to be sourced by a shell.

use super::*;

/// The staged archive one coordinate names.
pub fn archive_path(coordinate: &Coordinate, product: &str, platform: &str) -> String {
    format!(
        "{}/{product}/{}/{platform}/{product}.tar.gz",
        coordinate.local_root.trim_end_matches('/'),
        coordinate.version
    )
}

/// Resolve `$HOME`, `${HOME}` or a leading `~` in a path the env file declares.
///
/// The file is written to be sourced, so its values carry shell variables. The
/// path goes into a quoted argument here, where nothing expands it, so it is
/// expanded once against the host's real home and any OTHER variable is
/// refused rather than shipped as a literal that silently matches nothing.
pub fn expand_home(path: &str, home: &str) -> Result<String, DeployError> {
    let home = home.trim_end_matches('/');
    let expanded = if let Some(rest) = path.strip_prefix("~/") {
        format!("{home}/{rest}")
    } else if path == "~" {
        home.to_string()
    } else {
        path.replace("${HOME}", home).replace("$HOME", home)
    };
    if expanded.contains('$') {
        return Err(DeployError(format!(
            "the deployment env file declares path {path:?}, which this cannot resolve without \
             running a shell over it; replace it with $HOME, ${{HOME}}, ~, or an absolute path"
        ))
        .stating(crate::primitives::failure::FailureCode::Config));
    }
    Ok(expanded)
}
