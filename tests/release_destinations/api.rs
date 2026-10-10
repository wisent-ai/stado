use super::runner::{require, Journey};
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::fs;

async fn request(
    journey: &mut Journey,
    arguments: &[&str],
    confirmed: bool,
) -> Result<(StatusCode, Value), String> {
    let origin =
        url::Url::parse(&journey.configuration.api_origin).map_err(|error| error.to_string())?;
    let endpoint = origin
        .join("api/operator/run")
        .map_err(|error| error.to_string())?;
    let token = fs::read_to_string(&journey.configuration.api_token_file)
        .map_err(|error| error.to_string())?;
    require(
        !token.trim().is_empty(),
        "the dedicated native API token is empty",
    )?;
    let mut body = json!({"args": arguments});
    if confirmed {
        body["confirmation"] = json!("RUN_MUTATION");
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| error.to_string())?;
    let result = client
        .post(endpoint.clone())
        .header("X-Stado-Action", "operator-command")
        .bearer_auth(token.trim())
        .json(&body)
        .send()
        .await;
    if let Err(error) = &result {
        journey.report["commands"].as_array_mut().unwrap().push(json!({
            "endpoint": endpoint.as_str(), "request": body, "transport_error": error.to_string(),
        }));
        journey.save()?;
    }
    let response = result.map_err(|error| error.to_string())?;
    let status = response.status();
    let text = response.text().await.map_err(|error| error.to_string())?;
    journey.report["commands"].as_array_mut().unwrap().push(json!({
        "endpoint": endpoint.as_str(), "request": body, "http_status": status.as_u16(), "response": text,
    }));
    journey.save()?;
    let answer: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    Ok((status, answer))
}

pub async fn command(
    journey: &mut Journey,
    arguments: &[&str],
    confirmed: bool,
    refused: bool,
) -> Result<Value, String> {
    let (status, answer) = request(journey, arguments, confirmed).await?;
    let expected = if refused {
        StatusCode::FORBIDDEN
    } else {
        StatusCode::OK
    };
    require(
        status == expected,
        &format!("native API returned {status}: {answer}"),
    )?;
    if refused {
        return Ok(answer);
    }
    require(
        answer["ok"].as_bool() == Some(true)
            && answer["exit_code"].as_i64() == Some(i64::from(nix::libc::EXIT_SUCCESS)),
        &format!("native command failed: {answer}"),
    )?;
    require(
        answer["stdout_truncated"].as_bool() == Some(false)
            && answer["stderr_truncated"].as_bool() == Some(false),
        "native command evidence was truncated",
    )?;
    Ok(answer["structured"].clone())
}

/// Exercise the actual command failure response consumed by Desktop.
pub async fn failed_command(journey: &mut Journey, arguments: &[&str]) -> Result<Value, String> {
    let (status, answer) = request(journey, arguments, true).await?;
    require(
        status == StatusCode::OK,
        &format!("native command was not executed: HTTP {status}: {answer}"),
    )?;
    require(
        answer["ok"].as_bool() == Some(false)
            && answer["exit_code"]
                .as_i64()
                .is_some_and(|code| code != i64::from(nix::libc::EXIT_SUCCESS)),
        &format!("native command did not retain its actual failed exit: {answer}"),
    )?;
    require(
        answer["stdout_truncated"].as_bool() == Some(false)
            && answer["stderr_truncated"].as_bool() == Some(false),
        "native command failure evidence was truncated",
    )?;
    Ok(answer)
}
