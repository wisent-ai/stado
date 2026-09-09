//! Fold the remote program's `STADO_CRON` marker lines into one outcome.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

use crate::deploy::{host_channel, DeployError};

use super::CronOutcome;

/// Decode one base64 marker payload, or return it unchanged when a host
/// answered something this command cannot decode: a table is operator-facing
/// text, and dropping it on a decode error would hide the very content the
/// caller asked to see.
fn decode(payload: &str) -> String {
    STANDARD
        .decode(payload.trim().as_bytes())
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_else(|| payload.trim().to_string())
}

pub(super) fn parse(host: &str, stdout: &str) -> Result<CronOutcome, DeployError> {
    let mut outcome = CronOutcome {
        host: host.to_string(),
        ..CronOutcome::default()
    };
    let mut seen_state = false;
    for line in stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_CRON", state, detail] => {
                outcome.state = (*state).trim().to_string();
                outcome.detail = (*detail).trim().to_string();
                seen_state = true;
            }
            ["STADO_CRON_TABLE", payload] => {
                outcome.table = decode(payload)
                    .lines()
                    .filter(|row| !row.trim().is_empty())
                    .map(str::to_string)
                    .collect();
            }
            ["STADO_CRON_MATCH", payload] => outcome.matched.push(decode(payload)),
            ["STADO_CRON_BACKUP", path] => {
                outcome.backup_path = Some((*path).trim().to_string());
            }
            _ => {}
        }
    }
    if !seen_state {
        return Err(DeployError(format!(
            "{host}: the host reported no cron state"
        )));
    }
    Ok(outcome)
}
