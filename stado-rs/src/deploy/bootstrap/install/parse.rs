//! Stage one, readback: the three trailing stdout lines the install and
//! re-qualification scripts both agree to print.

/// Parse the remote install script's trailing output: platform, job-runtime
/// Python, then installed Stado path.
pub fn parse_remote_install(stdout: &str) -> (String, String, String) {
    let mut lines = stdout.trim().lines().rev();
    let stado_bin = lines.next().unwrap_or("").to_string();
    let wc_python = lines.next().unwrap_or("").to_string();
    let platform = lines.next().unwrap_or("").to_string();
    (platform, wc_python, stado_bin)
}
