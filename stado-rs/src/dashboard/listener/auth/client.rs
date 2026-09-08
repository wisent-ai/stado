//! Control-plane bearers: the exact service/action deployer grant and the
//! machine client one submit, status or cancel request authenticates as.

use serde_json::Value;

use crate::config;
use crate::dashboard::listener::http::Request;

use super::constant_time_eq;

pub(crate) async fn authorize_service(
    request: &Request,
    service: &str,
    action: &str,
) -> Result<bool, ()> {
    config::service_api_deployers().map_err(|_| ())?;
    let Some(policy) = config::service_deployer_for(service, action) else {
        return Ok(false);
    };
    let expected = crate::skarbiec::read_service_token(policy.item(), "token")
        .await
        .map_err(|_| ())?
        .filter(|value| !value.is_empty())
        .ok_or(())?;
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
    request: &Request,
    action: &str,
) -> Result<Option<&'static config::MachineApiClient>, ()> {
    let Some(supplied) = request
        .header("authorization")
        .and_then(|value| value.trim().strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let clients = config::machine_api_clients().map_err(|_| ())?;
    let mut matched = None;
    for client in clients
        .values()
        .filter(|client| client.allows_action(action))
    {
        let expected = crate::skarbiec::read_machine_token(client.item(), "token")
            .await
            .map_err(|_| ())?
            .filter(|value| !value.is_empty())
            .ok_or(())?;
        if constant_time_eq(expected.as_bytes(), supplied.as_bytes()) {
            if matched.is_some() {
                return Ok(None);
            }
            matched = Some(client);
        }
    }
    Ok(matched)
}
