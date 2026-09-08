//! Decoding a reporter's own stdout back into rows.

use crate::host_software::HostSoftware;

/// The reporter's stdout, as one row per program plus the script count.
///
/// Line-oriented `key=value` rather than JSON for the reason
/// `service_converge::parse_report` gives: a shell script that has to emit valid
/// JSON emits invalid JSON the first time a path contains a quote. Blank lines
/// and `#` comments are skipped and unknown keys are ignored, so the reporter can
/// add a field without a matching release here.
pub fn parse(stdout: &str) -> (Vec<HostSoftware>, usize) {
    let mut rows: Vec<HostSoftware> = Vec::new();
    let mut scripts = usize::default();
    for line in stdout.lines() {
        let line = line.trim();
        if let Some(body) = line.strip_prefix("software ") {
            // The wire order is the storage order, so one decoder serves both
            // and they cannot disagree about where `path=` begins. Only the name
            // is lifted out, because it is the half the fact name carries.
            let Some((_, rest)) = body.split_once("name=") else {
                continue;
            };
            let (name, rest) = rest.split_once(' ').unwrap_or((rest, ""));
            if name.is_empty() {
                continue;
            }
            if let Some(row) = HostSoftware::from_detail(name, rest) {
                rows.push(row);
            }
        } else if let Some(body) = line.strip_prefix("report ") {
            for token in body.split_whitespace() {
                if let Some(value) = token.strip_prefix("scripts=") {
                    scripts = value.parse().unwrap_or_default();
                }
            }
        }
    }
    (rows, scripts)
}
