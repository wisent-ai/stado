//! Which candidate path the host actually execed, read out of the stderr the
//! multi-candidate program marks it in.

use crate::deploy::DeployError;

use super::super::RESOLVED_EXECUTABLE_MARKER;

pub fn extract_resolved_executable(
    stderr: &mut String,
    candidates: &[&str],
) -> Result<Option<String>, DeployError> {
    let mut resolved: Option<String> = None;
    let mut retained = String::with_capacity(stderr.len());
    for segment in stderr.split_inclusive('\n') {
        // `lines` strips the `\n` or `\r\n` terminator the segment carries;
        // a segment is one line, so the first item is the whole of it.
        let line = match segment.lines().next() {
            Some(line) => line,
            None => "",
        };
        if let Some(path) = line.strip_prefix(RESOLVED_EXECUTABLE_MARKER) {
            if path.is_empty()
                || !candidates.contains(&path)
                || resolved.replace(path.to_string()).is_some()
            {
                return Err(DeployError::unreachable(
                    "host returned an invalid resolved executable marker".into(),
                ));
            }
        } else {
            retained.push_str(segment);
        }
    }
    let Some(resolved) = resolved else {
        return Ok(None);
    };
    *stderr = retained;
    Ok(Some(resolved))
}
