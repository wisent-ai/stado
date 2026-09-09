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
        )));
    }
    Ok(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_root_written_for_a_shell_is_resolved_once_against_the_real_home() {
        // charless-mac-mini's env file says exactly this, and a quoted argument
        // expands nothing - the first run of this verb looked for a directory
        // literally named $HOME.
        assert_eq!(
            expand_home("$HOME/.stado/releases/x.tar.gz", "/Users/charles"),
            Ok("/Users/charles/.stado/releases/x.tar.gz".to_string())
        );
        assert_eq!(
            expand_home("${HOME}/r", "/Users/charles/"),
            Ok("/Users/charles/r".to_string())
        );
        assert_eq!(
            expand_home("~/r", "/Users/charles"),
            Ok("/Users/charles/r".to_string())
        );
        let said = expand_home("$RELEASES/r", "/Users/charles")
            .unwrap_err()
            .to_string();
        assert!(said.contains("without running a shell over it"), "{said}");
    }
}
