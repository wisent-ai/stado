//! Stage one, readback: the two trailing stdout lines the install and
//! re-qualification scripts both agree to print.

/// Parse the remote install script's trailing output: platform, then the
/// installed Stado path.
pub fn parse_remote_install(stdout: &str) -> (String, String) {
    let mut lines = stdout.trim().lines().rev();
    let stado_bin = lines.next().unwrap_or("").to_string();
    let platform = lines.next().unwrap_or("").to_string();
    (platform, stado_bin)
}
