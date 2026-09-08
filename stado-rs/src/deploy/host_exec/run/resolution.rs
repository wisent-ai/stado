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
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(path) = line.strip_prefix(RESOLVED_EXECUTABLE_MARKER) {
            if path.is_empty()
                || !candidates.contains(&path)
                || resolved.replace(path.to_string()).is_some()
            {
                return Err(DeployError(
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
