//! Reading one key back through the channel that just wrote it.

use super::*;

/// Read one key back through the channel that just wrote it.
///
/// The comparison happens on the host against the same unquoting a shell would
/// apply, so `KEY='http://127.0.0.1:8895'` and `KEY=http://127.0.0.1:8895`
/// are one value and a secret is verified exactly without its value returning.
/// The forward markers are collected only when the write did not survive.
pub(super) async fn verify_env_write(
    target: &targets::ComputeTarget,
    env_file: &str,
    key: &str,
    value: &str,
    runner: &crate::deploy::Runner,
) -> Result<ReadBack, CmdError> {
    let request = service_env_file::EnvFileRequest {
        env_path: env_file,
        reveal: None,
        expect: Some((key, value)),
    };
    let report = service_env_file::read_env_file(target, &request, runner)
        .await
        .map_err(click)?;
    let state = match service_env_file::expectation(&report) {
        service_env_file::EXPECT_MATCHED => service_env_file::EXPECT_MATCHED,
        service_env_file::EXPECT_DIFFERS => service_env_file::EXPECT_DIFFERS,
        service_env_file::EXPECT_ABSENT => service_env_file::EXPECT_ABSENT,
        _ => service_env_file::EXPECT_UNVERIFIED,
    };
    let entry = service_env_file::effective_entry(&report, key);
    let effective = entry.and_then(|entry| {
        (entry.value_state != service_env_file::VALUE_REDACTED)
            .then(|| service_env_file::effective_text(&entry.value).to_string())
    });
    let mut marker = None;
    if state == service_env_file::EXPECT_DIFFERS {
        if let Some(observed) = effective.as_deref() {
            // Best effort: a channel that answered the read and not the
            // inventory must not turn "your write was overwritten" into a
            // failed command. The refusal below stands without attribution.
            if let Ok(markers) = service_env_file::forward_markers(target, runner).await {
                marker = service_env_file::marker_holding(&markers, observed).map(|found| {
                    format!("{} ($HOME/.stado/forwards/{}.url)", found.name, found.name)
                });
            }
        }
    }
    Ok(ReadBack {
        state,
        effective,
        chars: entry.map_or(u32::MIN, |entry| entry.chars),
        marker,
    })
}

pub(super) async fn verify_unit_env_write(
    target: &targets::ComputeTarget,
    declared: &service::ManagedService,
    key: &str,
    value: &str,
    runner: &crate::deploy::Runner,
) -> Result<ReadBack, CmdError> {
    let unit = service::fetch_unit_file(target, declared, runner)
        .await
        .map_err(click)?;
    let observed = service::parse_systemd_unit(&unit.content)
        .env
        .into_iter()
        .rev()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.replace("%%", "%"));
    let state = match observed.as_deref() {
        Some(found) if found == value => service_env_file::EXPECT_MATCHED,
        Some(_) => service_env_file::EXPECT_DIFFERS,
        None => service_env_file::EXPECT_ABSENT,
    };
    Ok(ReadBack {
        state,
        chars: observed
            .as_ref()
            .map_or(0, |value| value.chars().count() as u32),
        effective: observed.and_then(|value| {
            (service::redact_secret_value(key, &value) != service::REDACTED).then_some(value)
        }),
        marker: None,
    })
}
