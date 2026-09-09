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

fn header(response: &reqwest::Response, name: &str) -> String {
    response
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

fn named(value: &str, absent: &str) -> String {
    if value.is_empty() {
        absent.to_string()
    } else {
        value.to_string()
    }
}

/// Resolve the declared route, read the credential it names, and confront it
/// with GitHub. Exits non-zero when GitHub refuses that identity.
pub async fn report(json_output: bool) -> Result<(), CmdError> {
    let click = |error: String| CmdError::click(error).machine_readable(json_output);
    let identity = declared().map_err(click)?;
    let resolved = resolve().await.map_err(click)?;
    let credential = read(&resolved).await.map_err(click)?;
    let organization = crate::deploy::host_precheck_runner::GITHUB_ORGANIZATION;
    let endpoint = format!(
        "https://api.github.com{}",
        identity
            .reality_check
            .replace("{organization}", organization)
            .replace(
                "{repository}",
                identity
                    .reality_check_repository
                    .as_deref()
                    .unwrap_or_default()
            )
    );
    let response = reqwest::Client::new()
        .get(&endpoint)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header(reqwest::header::USER_AGENT, "wisent-stado-github-identity")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .bearer_auth(&credential)
        .send()
        .await
        .map_err(|error| click(format!("GitHub did not answer {endpoint}: {error}")))?;
    let status = response.status();
    let granted = header(&response, "x-oauth-scopes");
    let accepted = header(&response, "x-accepted-oauth-scopes");
    let bytes = response.bytes().await.unwrap_or_default();
    let answered = String::from_utf8_lossy(&bytes).replace(&credential, "[REDACTED]");
    let message = serde_json::from_str::<Value>(&answered)
        .ok()
        .and_then(|body| {
            body.get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default();
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
        println!("grants      {}", named(&granted, "-"));
    }
    if status.is_success() {
        return Ok(());
    }
    Err(click(format!(
        "the credential the declared GitHub route {:?} names, {}.{}, is not allowed on \
         {endpoint}: GitHub answered HTTP {} — {message}. That identity grants {}, and the \
         endpoint answers {}. Point {:?} at the intended credential with `skarbiec routes \
         add --resource {} --item <item> --field <field> --reason <text>`; the route is \
         declared in {DECLARATION_PATH}",
        resolved.route,
        resolved.item,
        resolved.field,
        status.as_u16(),
        named(&granted, "no listed permission"),
        named(&accepted, "an unstated permission"),
        resolved.route,
        resolved.route,
    )))
}
