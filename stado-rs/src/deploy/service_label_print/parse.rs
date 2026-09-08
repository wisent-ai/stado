//! The marker stream a host sends back, turned into a state. A partial
//! process-identity tuple is dropped rather than reported piecewise.

use super::state::{LabelReadFailure, LabelState};
use crate::deploy::host_channel;

/// Turn the marker stream into a state. Pure — covered by unit tests.
pub fn parse_label_print(host: &str, label: &str, stdout: &str) -> LabelState {
    let mut state = LabelState {
        host: host.to_string(),
        label: label.to_string(),
        ..LabelState::default()
    };
    for line in stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_LABEL_READ_FAILURE", domain, exit_code, detail] => {
                let domain = (*domain).trim();
                let detail = (*detail).trim();
                if !domain.is_empty() && !detail.is_empty() {
                    state.read_failures.push(LabelReadFailure {
                        domain: domain.to_string(),
                        exit_code: exit_code.trim().parse().unwrap_or(-1),
                        detail: detail.to_string(),
                    });
                }
            }
            ["STADO_LABEL_UNSUPPORTED", system] => {
                state.unsupported = Some((*system).trim().to_string());
            }
            ["STADO_LABEL_DOMAIN", domain] => {
                state.domain = Some((*domain).trim().to_string());
            }
            ["STADO_LABEL_FIELD", key, value] => {
                let value = (*value).trim().to_string();
                if value.is_empty() {
                    continue;
                }
                match (*key).trim() {
                    "pid" => {
                        state.pid = value
                            .parse::<u32>()
                            .ok()
                            .filter(|pid| *pid != 0)
                            .map(|pid| pid.to_string());
                    }
                    "state" => state.state = Some(value),
                    "last exit code" => state.last_exit_code = Some(value),
                    "runs" => state.runs = Some(value),
                    "path" => state.path = Some(value),
                    "program" => state.program = Some(value),
                    "arguments" => state.arguments = Some(value),
                    "process executable" => state.process_executable = Some(value),
                    "process device" => state.process_device = value.parse().ok(),
                    "process inode" => state.process_inode = value.parse().ok(),
                    "process start" => state.process_started_at = Some(value),
                    "process sha256" => state.process_sha256 = Some(value),
                    "stdout path" => state.stdout_path = Some(value),
                    "stderr path" => state.stderr_path = Some(value),
                    "unit file state" => state.unit_file_state = Some(value),
                    "restart" => state.restart = Some(value),
                    "triggers" => state.triggers = Some(value),
                    "triggered by" => state.triggered_by = Some(value),
                    "part of" => state.part_of = Some(value),
                    _ => {}
                }
            }
            ["STADO_LABEL_ENV", key, value] => {
                let key = (*key).trim();
                let value = (*value).trim();
                if matches!(
                    key,
                    "WC_STORAGE_BACKEND"
                        | "WC_LOCAL_STORAGE_PATH"
                        | "WC_BACKUP_STORAGE_BACKEND"
                        | "WC_BACKUP_LOCAL_STORAGE_PATH"
                        | "STADO_CONFIG"
                ) && !value.is_empty()
                {
                    state
                        .loaded_environment
                        .insert(key.to_string(), value.to_string());
                }
            }
            ["STADO_LABEL_IDENTITY_UNAVAILABLE", reason] => {
                let reason = (*reason).trim();
                if !reason.is_empty() {
                    state.process_identity_unavailable = Some(reason.to_string());
                }
            }
            ["STADO_LABEL_EVENT", event] => {
                let event = (*event).trim();
                if !event.is_empty() {
                    state.recent_events.push(event.to_string());
                }
            }
            ["STADO_LABEL_EVENT_STATUS", status] => {
                let status = (*status).trim();
                if !status.is_empty() {
                    state.event_read_status = Some(status.to_string());
                }
            }
            _ => {}
        }
    }
    let identity_fields = usize::from(state.process_executable.is_some())
        + usize::from(state.process_started_at.is_some())
        + usize::from(state.process_device.is_some())
        + usize::from(state.process_inode.is_some())
        + usize::from(state.process_sha256.is_some());
    let digest_valid = state.process_sha256.as_deref().is_none_or(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    });
    if identity_fields != 0
        && (identity_fields != 5 || state.process_inode == Some(0) || !digest_valid)
    {
        state.process_executable = None;
        state.process_started_at = None;
        state.process_device = None;
        state.process_inode = None;
        state.process_sha256 = None;
        state.process_identity_unavailable =
            Some("host returned an incomplete process identity tuple".to_string());
    }
    state
}
