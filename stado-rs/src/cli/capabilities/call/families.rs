//! The `image` and `search` families' requests and answers, and how their
//! alternatives are asked.
//!
//! - **image**: the request is `{"plan": EDIT_PLAN, "inputs": [PATH…],
//!   "output": PATH}`, an Obraz edit plan (`obraz operations` lists its
//!   steps; `generate` makes a picture from a description). Obraz runs it
//!   with `obraz edit --plan - --input … --output PATH --json
//!   --allow-provider-cost` and its answer is `obraz edit --json`'s: the
//!   output path, MIME type, width, height, bytes and steps. A request with a
//!   model step is billed by the image-model service Obraz's deployment
//!   names; the caller asked for it by sending one.
//! - **search**: the request is `{"query": TEXT, "count": N}`. The declared
//!   endpoint is sent it as `POST` JSON with the bearer of the item playing
//!   `capabilities.search.endpoint.key_role` and answers `{"results":
//!   [{"title", "url", "snippet"}…]}`; every `url` must be `http(s)://`.

use serde_json::{json, Value};

use super::Outcome;
use crate::capabilities::alternatives::Family;
use crate::capabilities::CapabilityKind;
use crate::cli::CmdError;

/// Refused before any alternative is asked: a request that is not the
/// family's shape.
pub(crate) fn check_request(family: &Family, request: &Value) -> Result<(), String> {
    match family.id {
        CapabilityKind::Inference => {
            if request["messages"].is_array() {
                Ok(())
            } else {
                Err("the request names no messages: standard input is an OpenAI chat-completions request".to_owned())
            }
        }
        CapabilityKind::Image => {
            if !request["plan"]["steps"].is_array() {
                return Err("the request names no plan: standard input is {\"plan\": {\"steps\": [...]}, \"inputs\": [...], \"output\": PATH}, an Obraz edit plan (`obraz operations` lists the steps)".to_owned());
            }
            if request["output"].as_str().is_none_or(|output| output.trim().is_empty()) {
                return Err("the request names no output: give \"output\", the path the picture is written to".to_owned());
            }
            if !(request["inputs"].is_null() || request["inputs"].as_array().is_some_and(|inputs| inputs.iter().all(Value::is_string))) {
                return Err("\"inputs\" is a list of picture paths, in the order the plan numbers them".to_owned());
            }
            Ok(())
        }
        CapabilityKind::Search => {
            if request["query"].as_str().is_none_or(|query| query.trim().is_empty()) {
                return Err("the request names no query: standard input is {\"query\": TEXT, \"count\": N}".to_owned());
            }
            if !request["count"].as_u64().is_some_and(|count| (1..=100).contains(&count)) {
                return Err("\"count\" is how many results to answer, a whole number from 1 to 100".to_owned());
            }
            Ok(())
        }
        other => Err(format!("{other} is resolved, not called")),
    }
}

/// Refused when an answer is not the family's shape, whoever gave it: an
/// alternative, or a statement being recorded.
pub(crate) fn check_answer(family: &Family, answer: &Value) -> Result<(), String> {
    match family.id {
        CapabilityKind::Image => {
            for field in ["output", "mime_type"] {
                if answer[field].as_str().is_none() {
                    return Err(format!("an image answer names its {field}: {{\"output\", \"mime_type\", \"width\", \"height\", \"bytes\", \"steps\"}}, as `obraz edit --json` answers"));
                }
            }
            for field in ["width", "height"] {
                if answer[field].as_u64().is_none_or(|pixels| pixels == 0) {
                    return Err(format!("an image answer gives its {field} in pixels"));
                }
            }
            Ok(())
        }
        CapabilityKind::Search => {
            let Some(results) = answer["results"].as_array() else {
                return Err("a search answer is {\"results\": [{\"title\", \"url\", \"snippet\"}...]}".to_owned());
            };
            for (index, result) in results.iter().enumerate() {
                let url = result["url"].as_str().unwrap_or_default();
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    return Err(format!("search result {index} names no http(s) address: {result}"));
                }
                if result["title"].as_str().is_none() {
                    return Err(format!("search result {index} names no title: {result}"));
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Obraz runs the request's plan.
pub(super) async fn obraz(program: &str, request: &Value) -> Outcome {
    let mut command = std::process::Command::new(program);
    command
        .args(["edit", "--plan", "-", "--json", "--allow-provider-cost", "--output"])
        .arg(request["output"].as_str().unwrap_or_default());
    for input in request["inputs"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        command.arg("--input").arg(input);
    }
    let plan = request["plan"].to_string();
    let ran = tokio::task::spawn_blocking(move || crate::wait::output_with_input(&mut command, plan.as_bytes())).await;
    let output = match ran {
        Ok(Ok(output)) => output,
        Ok(Err(error)) => return Outcome::Passed(format!("{program} could not be run: {error}")),
        Err(error) => return Outcome::Passed(format!("{program} could not be run: {error}")),
    };
    let said = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !output.status.success() {
        // Obraz's usage refusal (exit 2) is the request's fault and fails at
        // every alternative alike; any other failure is Obraz unable to
        // answer now.
        if output.status.code() == Some(2) {
            return Outcome::Refused(
                CmdError::refused(format!("Obraz refused the request itself: {said}"))
                    .detailing(json!({"provider": "obraz", "stderr": said})),
            );
        }
        return Outcome::Passed(format!("{program} edit failed ({}): {said}", output.status));
    }
    match serde_json::from_slice::<Value>(&output.stdout) {
        Ok(answer) => Outcome::Answered(answer),
        Err(error) => Outcome::Passed(format!(
            "{program} edit answered no JSON ({error}): {}",
            String::from_utf8_lossy(&output.stdout).trim()
        )),
    }
}

/// The declared search endpoint is sent the query.
pub(super) async fn endpoint(route: &str, key_role: &crate::capabilities::ConfigField, body: &str) -> Outcome {
    let role = crate::config_file::field_value(key_role)
        .and_then(|value| value.as_str().map(str::to_owned))
        .filter(|role| !role.trim().is_empty());
    let bearer = match &role {
        Some(role) => match super::role_field(role, "token").await {
            Ok(Some(token)) => Some(token),
            Ok(None) => {
                return Outcome::Passed(format!(
                    "{} names role {role}, and no item plays it; store the endpoint's key with `stado credentials item put --host <vault owner> --role {role}`",
                    key_role.path
                ))
            }
            Err(cause) => return Outcome::Passed(format!("the search endpoint's key (role {role}): {cause}")),
        },
        None => None,
    };
    let client = match reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build() {
        Ok(client) => client,
        Err(error) => return Outcome::Passed(crate::cli::http_failure(&error)),
    };
    let mut request = client.post(route).header("content-type", "application/json");
    if let Some(bearer) = bearer {
        request = request.bearer_auth(bearer.trim());
    }
    let response = match crate::wait::request(request.body(body.to_owned())).await {
        Ok(response) => response,
        Err(error) => return Outcome::Passed(format!("{route}: {}", crate::cli::http_failure(&error))),
    };
    let status = response.status();
    let text = match response.text().await {
        Ok(text) => text,
        Err(error) => {
            return Outcome::Passed(format!(
                "{route} answered {status} with a body that cannot be read: {}",
                crate::cli::http_failure(&error)
            ))
        }
    };
    if !status.is_success() {
        return Outcome::Passed(format!("{route} answered {status}: {}", text.trim()));
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(answer) => Outcome::Answered(answer),
        Err(error) => Outcome::Passed(format!("{route} answered {status} with a body that is not JSON ({error}): {}", text.trim())),
    }
}
