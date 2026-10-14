//! Stage one, readback: the two trailing stdout lines the install and
//! re-qualification scripts both agree to print.

/// Parse the remote install script's trailing output: platform, then the
/// installed Stado path. `None` when the output does not end in two
/// non-empty lines: a report without them is a damaged answer, not one
/// naming an empty platform or path.
pub fn parse_remote_install(stdout: &str) -> Option<(String, String)> {
    let mut lines = stdout.trim().lines().rev();
    let stado_bin = lines
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())?;
    let platform = lines
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())?;
    Some((platform.to_string(), stado_bin.to_string()))
}
