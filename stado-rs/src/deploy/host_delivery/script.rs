//! The delivery report protocol: the guard lines every host-side script
//! shares, and the marker line those scripts answer with.

use crate::deploy::{host_channel, shlex_quote, DeployError};
use crate::targets::ComputeTarget;

pub const DELIVERED_STATUS: &str = "delivered";
pub(super) const MARKER: &str = "STADO_DELIVER";

pub(super) fn guard_lines(home: &str, components: &[&str], include_destination: bool) -> String {
    let mut lines = String::new();
    let count = if include_destination {
        components.len()
    } else {
        components.len().saturating_sub(1)
    };
    for index in 0..count {
        let path = format!("{home}/{}", components[..=index].join("/"));
        let quoted = shlex_quote(&path);
        lines.push_str(&format!(
            "if [ -L {quoted} ]; then report refused {}; exit 0; fi\n",
            shlex_quote(&format!("destination traverses a symlink at {path}"))
        ));
        if index + 1 < components.len() {
            lines.push_str(&format!(
                "if [ -e {quoted} ]; then [ -d {quoted} ] || {{ report refused {}; exit 0; }}; [ -O {quoted} ] || {{ report refused {}; exit 0; }}; else /bin/mkdir {quoted}; /bin/chmod 700 {quoted}; fi\n",
                shlex_quote(&format!("destination parent is not a directory: {path}")),
                shlex_quote(&format!("destination parent is not owned by the approved account: {path}")),
            ));
        }
    }
    lines
}

pub(super) fn parse_marker(
    target: &ComputeTarget,
    output: &crate::deploy::CommandOutput,
) -> Result<(String, String), DeployError> {
    let fields = output
        .stdout
        .lines()
        .find_map(|line| {
            let fields = host_channel::marker_fields(line);
            (fields.first().copied() == Some(MARKER)).then_some(fields)
        })
        .ok_or_else(|| {
            DeployError(format!(
                "{}: the host answered without a delivery report: {}",
                target.name,
                host_channel::last_error_line(output, "no marker in output")
            ))
        })?;
    let status = fields.get(1).copied().unwrap_or_default().to_string();
    let detail = fields.get(2).copied().unwrap_or_default().to_string();
    Ok((status, detail))
}
