//! The reality check behind the declaration.
//!
//! A route that resolves proves a credential exists. What makes the declaration
//! true is GitHub accepting that identity on the exact endpoint the runner
//! lifecycle needs, so this reads it and prints the status, GitHub's own
//! message and the scope sets GitHub named. The value is used and never
//! printed: the answer is a status and a coordinate.

use serde_json::{json, Value};

use super::{declared, read, resolve, DECLARATION_PATH};
use crate::cli::CmdError;

/// A response header as GitHub sent it; none when GitHub sent no such header. A value
/// that is not visible ASCII is shown with its bytes read as UTF-8.
fn header(response: &reqwest::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get(name)
        .map(|value| String::from_utf8_lossy(value.as_bytes()).into_owned())
}

/// `value` for a person to read, or `absent` when GitHub stated none.
fn named(value: Option<&str>, absent: &str) -> String {
    match value {
        Some(value) if !value.is_empty() => value.to_string(),
        _ => absent.to_string(),
    }
}

/// Resolve the declared route, read the credential it names, and confront it
/// with GitHub. Exits non-zero when GitHub refuses that identity.
pub async fn report(json_output: bool) -> Result<(), CmdError> {
    use crate::primitives::failure::FailureCode;
    let click = |error: String| CmdError::click(error).machine_readable(json_output);
    // The declaration is the fleet's configuration; the vault lookups keep
    // the class each failure stated where it was raised.
    let identity = declared().map_err(|error| click(error).stating(FailureCode::Config))?;
    let classed =
        |error: crate::deploy::DeployError| CmdError::from(error).machine_readable(json_output);
    let resolved = resolve().await.map_err(classed)?;
    let credential = read(&resolved).await.map_err(classed)?;
    let organization = crate::deploy::host_precheck_runner::GITHUB_ORGANIZATION;
    let mut path = identity
        .reality_check
        .replace("{organization}", organization);
    if path.contains("{repository}") {
        let repository = identity.reality_check_repository.as_deref().ok_or_else(|| {
            click(format!(
                "{DECLARATION_PATH}: reality_check {} names {{repository}}, but no reality_check_repository is declared",
                identity.reality_check
            ))
            .stating(FailureCode::Config)
        })?;
        path = path.replace("{repository}", repository);
    }
    let endpoint = format!("https://api.github.com{path}");
    let response = crate::wait::request(
        reqwest::Client::new()
            .get(&endpoint)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header(reqwest::header::USER_AGENT, "wisent-stado-github-identity")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .bearer_auth(&credential),
    )
    .await
    .map_err(|error| {
        let mut unanswered = click(format!("GitHub did not answer {endpoint}: {error}"));
        unanswered.failure = CmdError::from(error).failure;
        unanswered
    })?;
    let status = response.status();
    let granted = header(&response, "x-oauth-scopes");
    let accepted = header(&response, "x-accepted-oauth-scopes");
    let bytes = response.bytes().await.map_err(|error| {
        let mut unread = click(format!(
            "GitHub answered {endpoint} with HTTP {}, but its body could not be read: {error}",
            status.as_u16()
        ));
        unread.failure = CmdError::from(error).failure;
        unread
    })?;
    let answered = String::from_utf8_lossy(&bytes).replace(&credential, "[REDACTED]");
    // GitHub states why it refused in the body's `message`; a body that is not JSON
    // is itself what GitHub said.
    let message: Option<String> = match serde_json::from_str::<Value>(&answered) {
        Ok(body) => body
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string),
        Err(not_json) => Some(format!("a body that is not JSON ({not_json}): {answered}")),
    };
    let document = json!({
        "accepted": status.is_success(),
        "accepted_permissions": accepted,
        "declaration": DECLARATION_PATH,
        "declared_by": resolved.declared_by,
        "field": resolved.field,
        "github_message": message,
        "granted_permissions": granted,
        "item": resolved.item,
        "organization": organization,
        "reality_check": endpoint,
        "required_permission": identity.required_permission,
        "route": resolved.route,
        "status": status.as_u16(),
    });
    if json_output {
        println!(
            "{}",
            crate::deploy::host_recovery::to_sorted_pretty(&document)
        );
    } else {
        println!("route       {}", resolved.route);
        println!("coordinate  {}.{}", resolved.item, resolved.field);
        println!("declared by {}", resolved.declared_by);
        println!("check       {endpoint}");
        println!("status      {}", status.as_u16());
        println!("requires    {}", identity.required_permission);
        println!("grants      {}", named(granted.as_deref(), "-"));
    }
    if status.is_success() {
        return Ok(());
    }
    Err(click(format!(
        "the credential the declared GitHub route {:?} names, {}.{}, is not allowed on \
         {endpoint}: GitHub answered HTTP {} — {}. That identity grants {}, and the \
         endpoint answers {}. Point {:?} at the intended credential with `skarbiec route \
         declare --resource {} --item <item> --field <field> --reason <text>`; the route is \
         declared in {DECLARATION_PATH}",
        resolved.route,
        resolved.item,
        resolved.field,
        status.as_u16(),
        named(message.as_deref(), "with no message"),
        named(granted.as_deref(), "no listed permission"),
        named(accepted.as_deref(), "an unstated permission"),
        resolved.route,
        resolved.route,
    ))
    .stating(FailureCode::Auth))
}
