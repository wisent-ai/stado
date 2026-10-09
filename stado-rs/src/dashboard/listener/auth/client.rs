//! Control-plane bearers: the exact service/action deployer grant and the
//! machine client one submit, status or cancel request authenticates as.
//! Each is read from the vault on every request, as one consultation
//! (`super::vault`): a vault that does not answer refuses the request with
//! its wait line instead of holding it.

use serde_json::Value;

use crate::config;
use crate::dashboard::listener::http::Request;
use crate::dashboard::listener::Dashboard;

use super::{constant_time_eq, AuthorityUnavailable};

pub(crate) async fn authorize_service(
    dashboard: &Dashboard,
    request: &Request,
    service: &str,
    action: &str,
) -> Result<bool, AuthorityUnavailable> {
    config::service_api_deployers().map_err(|refusals| {
        AuthorityUnavailable::new(format!(
            "service_api.deployers is refused: {}",
            refusals.join("; ")
        ))
    })?;
    let Some(policy) = config::service_deployer_for(service, action) else {
        return Ok(false);
    };
    let item = policy.item();
    let expected = dashboard
        .consult_vault(
            &format!("service:{item}"),
            false,
            format!("the token of service deployer item {item}"),
            crate::skarbiec::read_service_token(item, "token"),
        )
        .await?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AuthorityUnavailable::new(format!("service deployer item {item} holds no token"))
        })?;
    let authorization = request.header("authorization").unwrap_or("").trim();
    let supplied = authorization.strip_prefix("Bearer ").unwrap_or_default();
    Ok(constant_time_eq(expected.as_bytes(), supplied.as_bytes()))
}

pub(crate) fn machine_result_target(value: &Value) -> Option<&str> {
    value
        .get("job")
        .and_then(Value::as_object)
        .and_then(|job| job.get("provider"))
        .and_then(Value::as_str)
        .filter(|target| !target.is_empty())
}

pub(crate) async fn authenticate_machine_client(
    dashboard: &Dashboard,
    request: &Request,
    action: &str,
) -> Result<Option<&'static config::MachineApiClient>, AuthorityUnavailable> {
    let Some(supplied) = request
        .header("authorization")
        .and_then(|value| value.trim().strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let clients = config::machine_api_clients().map_err(|refusals| {
        AuthorityUnavailable::new(format!(
            "machine_api.clients is refused: {}",
            refusals.join("; ")
        ))
    })?;
    let mut matched = None;
    for client in clients
        .values()
        .filter(|client| client.allows_action(action))
    {
        let item = client.item();
        let expected = dashboard
            .consult_vault(
                &format!("machine:{item}"),
                false,
                format!("the token of machine client item {item}"),
                crate::skarbiec::read_machine_token(item, "token"),
            )
            .await?
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AuthorityUnavailable::new(format!("machine client item {item} holds no token"))
            })?;
        if constant_time_eq(expected.as_bytes(), supplied.as_bytes()) {
            if matched.is_some() {
                return Ok(None);
            }
            matched = Some(client);
        }
    }
    Ok(matched)
}
