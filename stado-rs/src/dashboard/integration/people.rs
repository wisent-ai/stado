use reqwest::{Client, Method, StatusCode};
use serde_json::{json, Map, Value};
use std::time::Duration;

use super::{provider_client, HandlerError, HandlerResult};

const GITHUB_ITEM: &str = "people-rotator-github-admin";
const SLACK_ITEM: &str = "people-rotator-slack-admin";
const SUPABASE_ITEM: &str = "people-rotator-supabase-admin";
const WELES_ITEM: &str = "people-rotator-weles-queue";

const ACTIONS: &[&str] = &[
    "prerequisites",
    "github.org.invite_member",
    "github.team.add_member",
    "github.repo.remove_collaborator",
    "github.org.remove_member",
    "github.membership.check",
    "github.org.revoke_fine_grained_pat_grants",
    "github.repo.transfer",
    "slack.user.invite",
    "slack.user.deactivate",
    "supabase.auth.invite_user",
    "supabase.auth.ban_user",
    "supabase.credentials.rotate",
    "weles.queue.enqueue",
];

pub(super) fn supports(action: &str) -> bool {
    ACTIONS.contains(&action)
}

fn client() -> Result<Client, HandlerError> {
    Client::builder()
        .connect_timeout(Duration::from_secs("10".parse().expect("static timeout")))
        .timeout(Duration::from_secs("45".parse().expect("static timeout")))
        .pool_idle_timeout(Duration::from_secs("30".parse().expect("static timeout")))
        .user_agent("stado-people-integration")
        .build()
        .map_err(|_| HandlerError::ProviderUnavailable)
}

fn object(body: &[u8]) -> Result<Map<String, Value>, HandlerError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    value.as_object().cloned().ok_or(HandlerError::BadRequest)
}

fn exact_keys(map: &Map<String, Value>, allowed: &[&str]) -> Result<(), HandlerError> {
    if map.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(HandlerError::BadRequest);
    }
    Ok(())
}

fn required_string(map: &Map<String, Value>, key: &str) -> Result<String, HandlerError> {
    let value = map
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(HandlerError::BadRequest)?;
    if value.len() > "512".parse::<usize>().expect("static bound") {
        return Err(HandlerError::BadRequest);
    }
    Ok(value.to_string())
}

fn optional_string(map: &Map<String, Value>, key: &str) -> Result<Option<String>, HandlerError> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value))
            if !value.trim().is_empty()
                && value.len() <= "2048".parse::<usize>().expect("static bound") =>
        {
            Ok(Some(value.trim().to_string()))
        }
        _ => Err(HandlerError::BadRequest),
    }
}

fn strings(map: &Map<String, Value>, key: &str) -> Result<Vec<String>, HandlerError> {
    let values = map
        .get(key)
        .and_then(Value::as_array)
        .ok_or(HandlerError::BadRequest)?;
    if values.len() > "100".parse::<usize>().expect("static bound") {
        return Err(HandlerError::BadRequest);
    }
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::trim)
                .filter(|value| {
                    !value.is_empty()
                        && value.len() <= "512".parse::<usize>().expect("static bound")
                })
                .map(str::to_string)
                .ok_or(HandlerError::BadRequest)
        })
        .collect()
}

fn segment(value: &str) -> Result<String, HandlerError> {
    if value.is_empty()
        || value.len() > "128".parse::<usize>().expect("static bound")
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    {
        return Err(HandlerError::BadRequest);
    }
    Ok(value.to_string())
}

fn repo(value: &str) -> Result<String, HandlerError> {
    let (owner, name) = value.split_once('/').ok_or(HandlerError::BadRequest)?;
    if name.contains('/') {
        return Err(HandlerError::BadRequest);
    }
    Ok(format!("{}/{}", segment(owner)?, segment(name)?))
}

async fn response_json(
    builder: reqwest::RequestBuilder,
) -> Result<(StatusCode, Value), HandlerError> {
    let response = builder
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let body = if text.is_empty() {
        json!({})
    } else {
        serde_json::from_str(&text).map_err(|_| HandlerError::UpstreamFailure)?
    };
    Ok((status, body))
}

async fn github_credentials(app: bool) -> Result<String, HandlerError> {
    let provider = provider_client("people").await?;
    provider
        .read_string(GITHUB_ITEM, if app { "app_token" } else { "token" })
        .await
}

async fn github_request(
    method: Method,
    path: &str,
    token: &str,
    body: Option<Value>,
) -> Result<(StatusCode, Value), HandlerError> {
    let mut request = client()?
        .request(method, format!("https://api.github.com{path}"))
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    if let Some(body) = body {
        request = request.json(&body);
    }
    response_json(request).await
}

fn successful(status: StatusCode) -> Result<(), HandlerError> {
    if status.is_success() {
        Ok(())
    } else {
        Err(HandlerError::UpstreamFailure)
    }
}

async fn github_org_action(action: &str, body: &[u8]) -> HandlerResult {
    let map = object(body)?;
    exact_keys(&map, &["github_user", "github_orgs", "expect"])?;
    let user = segment(&required_string(&map, "github_user")?)?;
    let orgs = strings(&map, "github_orgs")?
        .into_iter()
        .map(|org| segment(&org))
        .collect::<Result<Vec<_>, _>>()?;
    let expect = optional_string(&map, "expect")?;
    let token = github_credentials(false).await?;
    let mut events = Vec::new();
    for org in orgs {
        let membership = format!("/orgs/{org}/memberships/{user}");
        match action {
            "github.org.invite_member" => {
                let (status, value) = github_request(
                    Method::PUT,
                    &membership,
                    &token,
                    Some(json!({"role": "member"})),
                )
                .await?;
                successful(status)?;
                events.push(json!({"status": value.get("state").and_then(Value::as_str).unwrap_or("invited"), "org": org, "user": user}));
            }
            "github.org.remove_member" => {
                let member_path = format!("/orgs/{org}/members/{user}");
                let (status, _) =
                    github_request(Method::DELETE, &member_path, &token, None).await?;
                if status.is_success() {
                    events.push(json!({"status": "removed", "org": org, "user": user}));
                } else if status == StatusCode::NOT_FOUND {
                    let (list_status, invitations) = github_request(
                        Method::GET,
                        &format!("/orgs/{org}/invitations"),
                        &token,
                        None,
                    )
                    .await?;
                    successful(list_status)?;
                    let invitation = invitations.as_array().and_then(|rows| {
                        rows.iter().find(|row| {
                            row.get("login").and_then(Value::as_str) == Some(user.as_str())
                                || row
                                    .get("invitee")
                                    .and_then(|value| value.get("login"))
                                    .and_then(Value::as_str)
                                    == Some(user.as_str())
                        })
                    });
                    if let Some(id) = invitation
                        .and_then(|value| value.get("id"))
                        .and_then(Value::as_u64)
                    {
                        let (cancel_status, _) = github_request(
                            Method::DELETE,
                            &format!("/orgs/{org}/invitations/{id}"),
                            &token,
                            None,
                        )
                        .await?;
                        successful(cancel_status)?;
                        events.push(json!({"status": "cancelled-invite", "org": org, "user": user, "invitation_id": id}));
                    } else {
                        events.push(json!({"status": "not-found", "org": org, "user": user}));
                    }
                } else {
                    return Err(HandlerError::UpstreamFailure);
                }
            }
            "github.membership.check" => {
                let (status, value) =
                    github_request(Method::GET, &membership, &token, None).await?;
                if expect.as_deref() == Some("not_found") {
                    if status != StatusCode::NOT_FOUND {
                        return Err(HandlerError::Conflict);
                    }
                    events.push(json!({"status": "denied", "org": org, "user": user}));
                } else {
                    successful(status)?;
                    let state = value
                        .get("state")
                        .and_then(Value::as_str)
                        .ok_or(HandlerError::UpstreamFailure)?;
                    if !matches!(state, "active" | "pending") {
                        return Err(HandlerError::Conflict);
                    }
                    events.push(json!({"status": "checked", "org": org, "user": user, "state": state, "role": value.get("role")}));
                }
            }
            _ => return Err(HandlerError::BadRequest),
        }
    }
    Ok(json!({"events": events}))
}

async fn github_team_add(body: &[u8]) -> HandlerResult {
    let map = object(body)?;
    exact_keys(&map, &["github_user", "github_orgs", "github_teams"])?;
    let user = segment(&required_string(&map, "github_user")?)?;
    let orgs = strings(&map, "github_orgs")?
        .into_iter()
        .map(|value| segment(&value))
        .collect::<Result<Vec<_>, _>>()?;
    let teams = strings(&map, "github_teams")?
        .into_iter()
        .map(|value| segment(&value))
        .collect::<Result<Vec<_>, _>>()?;
    let token = github_credentials(false).await?;
    let mut events = Vec::new();
    for org in orgs {
        for team in &teams {
            let (status, _) = github_request(
                Method::PUT,
                &format!("/orgs/{org}/teams/{team}/memberships/{user}"),
                &token,
                Some(json!({"role": "member"})),
            )
            .await?;
            successful(status)?;
            events.push(json!({"status": "added", "org": org, "team": team, "user": user}));
        }
    }
    Ok(json!({"events": events}))
}

async fn github_repo_remove(body: &[u8]) -> HandlerResult {
    let map = object(body)?;
    exact_keys(&map, &["github_user", "github_repos"])?;
    let user = segment(&required_string(&map, "github_user")?)?;
    let repos = strings(&map, "github_repos")?
        .into_iter()
        .map(|value| repo(&value))
        .collect::<Result<Vec<_>, _>>()?;
    let token = github_credentials(false).await?;
    let mut events = Vec::new();
    for repository in repos {
        let (status, _) = github_request(
            Method::DELETE,
            &format!("/repos/{repository}/collaborators/{user}"),
            &token,
            None,
        )
        .await?;
        if !status.is_success() && status != StatusCode::NOT_FOUND {
            return Err(HandlerError::UpstreamFailure);
        }
        events.push(json!({"status": if status == StatusCode::NOT_FOUND { "not-found" } else { "removed" }, "repo": repository, "user": user}));
    }
    Ok(json!({"events": events}))
}

async fn github_pat_revoke(body: &[u8]) -> HandlerResult {
    let map = object(body)?;
    exact_keys(&map, &["github_user", "github_orgs", "github_pat_ids"])?;
    let user = segment(&required_string(&map, "github_user")?)?;
    let orgs = strings(&map, "github_orgs")?
        .into_iter()
        .map(|value| segment(&value))
        .collect::<Result<Vec<_>, _>>()?;
    let requested = strings(&map, "github_pat_ids")?;
    let token = github_credentials(true).await?;
    let mut events = Vec::new();
    for org in orgs {
        let path = format!("/orgs/{org}/personal-access-tokens?owner={user}");
        let (status, listed) = github_request(Method::GET, &path, &token, None).await?;
        successful(status)?;
        let mut ids = requested
            .iter()
            .filter_map(|value| value.parse::<u64>().ok())
            .collect::<Vec<_>>();
        if ids.is_empty() {
            ids = listed
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|value| value.get("id").and_then(Value::as_u64))
                .collect();
        }
        if ids.is_empty() {
            events.push(
                json!({"status": "none", "org": org, "user": user, "grant_count": ids.len()}),
            );
            continue;
        }
        let (revoke_status, _) = github_request(
            Method::POST,
            &format!("/orgs/{org}/personal-access-tokens"),
            &token,
            Some(json!({"action": "revoke", "pat_ids": ids})),
        )
        .await?;
        successful(revoke_status)?;
        events
            .push(json!({"status": "revoked", "org": org, "user": user, "grant_count": ids.len()}));
    }
    Ok(json!({"events": events}))
}

async fn github_transfer(body: &[u8]) -> HandlerResult {
    let map = object(body)?;
    exact_keys(&map, &["github_repos", "new_owner", "team_ids"])?;
    let repos = strings(&map, "github_repos")?
        .into_iter()
        .map(|value| repo(&value))
        .collect::<Result<Vec<_>, _>>()?;
    let new_owner = segment(&required_string(&map, "new_owner")?)?;
    let team_ids = strings(&map, "team_ids")?
        .into_iter()
        .map(|value| value.parse::<u64>().map_err(|_| HandlerError::BadRequest))
        .collect::<Result<Vec<_>, _>>()?;
    let token = github_credentials(false).await?;
    let mut events = Vec::new();
    for repository in repos {
        let (status, _) = github_request(
            Method::POST,
            &format!("/repos/{repository}/transfer"),
            &token,
            Some(json!({"new_owner": new_owner, "team_ids": team_ids})),
        )
        .await?;
        successful(status)?;
        events.push(json!({"status": "queued", "repo": repository, "new_owner": new_owner, "team_ids": team_ids}));
    }
    Ok(json!({"events": events}))
}

async fn slack(action: &str, body: &[u8]) -> HandlerResult {
    let map = object(body)?;
    let provider = provider_client("people").await?;
    let token = provider.read_string(SLACK_ITEM, "token").await?;
    let email = required_string(&map, "person_email")?;
    let lookup = client()?
        .get("https://slack.com/api/users.lookupByEmail")
        .bearer_auth(&token)
        .query(&[("email", email.as_str())]);
    let (lookup_status, found) = response_json(lookup).await?;
    successful(lookup_status)?;
    if action == "slack.user.deactivate" {
        exact_keys(&map, &["person_email", "team_id"])?;
        if found.get("ok") != Some(&Value::Bool(true)) {
            if found.get("error").and_then(Value::as_str) == Some("users_not_found") {
                return Ok(json!({"status": "not-found"}));
            }
            return Err(HandlerError::UpstreamFailure);
        }
        let user_id = found
            .get("user")
            .and_then(|value| value.get("id"))
            .and_then(Value::as_str)
            .ok_or(HandlerError::UpstreamFailure)?;
        let team_id = optional_string(&map, "team_id")?;
        let mut form = vec![("user_id", user_id.to_string())];
        if let Some(team_id) = team_id {
            form.push(("team_id", team_id));
        }
        let (status, removed) = response_json(
            client()?
                .post("https://slack.com/api/admin.users.remove")
                .bearer_auth(&token)
                .form(&form),
        )
        .await?;
        successful(status)?;
        if removed.get("ok") != Some(&Value::Bool(true)) {
            return Err(HandlerError::UpstreamFailure);
        }
        return Ok(json!({"status": "deactivated", "user_id": user_id}));
    }
    exact_keys(
        &map,
        &["person_email", "person_name", "team_id", "channel_ids"],
    )?;
    if found.get("ok") == Some(&Value::Bool(true)) {
        return Ok(
            json!({"status": "already-exists", "user_id": found.get("user").and_then(|value| value.get("id"))}),
        );
    }
    if found.get("error").and_then(Value::as_str) != Some("users_not_found") {
        return Err(HandlerError::UpstreamFailure);
    }
    let team_id = required_string(&map, "team_id")?;
    let channel_ids = strings(&map, "channel_ids")?.join(",");
    let mut form = vec![
        ("email", email),
        ("team_id", team_id),
        ("channel_ids", channel_ids),
        ("resend", "true".to_string()),
    ];
    if let Some(name) = optional_string(&map, "person_name")? {
        form.push(("real_name", name));
    }
    let (status, invited) = response_json(
        client()?
            .post("https://slack.com/api/admin.users.invite")
            .bearer_auth(&token)
            .form(&form),
    )
    .await?;
    successful(status)?;
    if invited.get("ok") != Some(&Value::Bool(true)) {
        if matches!(
            invited.get("error").and_then(Value::as_str),
            Some("already_in_team" | "already_in_team_invited_user")
        ) {
            return Ok(json!({"status": "already-invited"}));
        }
        return Err(HandlerError::UpstreamFailure);
    }
    Ok(json!({"status": "invited"}))
}

fn provider_origin(value: &str) -> Result<String, HandlerError> {
    let parsed = url::Url::parse(value).map_err(|_| HandlerError::ProviderUnavailable)?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !matches!(parsed.path(), "" | "/")
    {
        return Err(HandlerError::ProviderUnavailable);
    }
    Ok(parsed.origin().ascii_serialization())
}

async fn supabase_credentials(
) -> Result<(super::ProviderClient, String, String, String), HandlerError> {
    let provider = provider_client("people").await?;
    let url = provider.read_string(SUPABASE_ITEM, "url").await?;
    let project_ref = provider.read_string(SUPABASE_ITEM, "project_ref").await?;
    let access_token = provider.read_string(SUPABASE_ITEM, "access_token").await?;
    let origin = provider_origin(&url)?;
    Ok((provider, origin, project_ref, access_token))
}

async fn find_supabase_user(
    http: &Client,
    url: &str,
    key: &str,
    email: &str,
) -> Result<Option<String>, HandlerError> {
    let (status, listed) = response_json(
        http.get(format!("{url}/auth/v1/admin/users?page=1&per_page=1000"))
            .header("apikey", key)
            .bearer_auth(key),
    )
    .await?;
    successful(status)?;
    Ok(listed
        .get("users")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|user| {
            user.get("email")
                .and_then(Value::as_str)
                .is_some_and(|value| value.eq_ignore_ascii_case(email))
        })
        .and_then(|user| user.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string))
}

async fn supabase_auth(action: &str, body: &[u8]) -> HandlerResult {
    let map = object(body)?;
    let (provider, url, _, _) = supabase_credentials().await?;
    let key = provider
        .read_string(SUPABASE_ITEM, "service_role_key")
        .await?;
    let email = required_string(&map, "person_email")?;
    let http = client()?;
    let existing = find_supabase_user(&http, &url, &key, &email).await?;
    if action == "supabase.auth.ban_user" {
        exact_keys(&map, &["person_email", "user_id"])?;
        let user_id = optional_string(&map, "user_id")?.or(existing);
        let Some(user_id) = user_id else {
            return Ok(json!({"status": "not-found"}));
        };
        let (status, _) = response_json(
            http.put(format!("{url}/auth/v1/admin/users/{}", segment(&user_id)?))
                .header("apikey", &key)
                .bearer_auth(&key)
                .json(&json!({"ban_duration": "876000h"})),
        )
        .await?;
        successful(status)?;
        return Ok(json!({"status": "banned", "user_id": user_id}));
    }
    exact_keys(
        &map,
        &["person_email", "person_name", "github_user", "redirect_to"],
    )?;
    if let Some(user_id) = existing {
        return Ok(json!({"status": "already-exists", "user_id": user_id}));
    }
    let mut invite = json!({"email": email, "data": {}});
    if let Some(name) = optional_string(&map, "person_name")? {
        invite["data"]["full_name"] = Value::String(name);
    }
    if let Some(user) = optional_string(&map, "github_user")? {
        invite["data"]["github_user"] = Value::String(user);
    }
    if let Some(redirect) = optional_string(&map, "redirect_to")? {
        invite["redirect_to"] = Value::String(redirect);
    }
    let (status, invited) = response_json(
        http.post(format!("{url}/auth/v1/invite"))
            .header("apikey", &key)
            .bearer_auth(&key)
            .json(&invite),
    )
    .await?;
    successful(status)?;
    Ok(
        json!({"status": "invited", "user_id": invited.get("id").or_else(|| invited.get("user").and_then(|value| value.get("id")))}),
    )
}

async fn write_provider_item(item: &str, value: &Value) -> Result<(), HandlerError> {
    crate::skarbiec::validate_integration_provider("people")
        .await
        .map_err(|_| HandlerError::ProviderUnavailable)?;
    let policy =
        crate::config::integration_provider("people").ok_or(HandlerError::ProviderUnavailable)?;
    if !policy.items().iter().any(|allowed| allowed == item) {
        return Err(HandlerError::ProviderUnavailable);
    }
    crate::skarbiec::Client::integration_provider("people")
        .map_err(|_| HandlerError::ProviderUnavailable)?
        .write_item(item, "api_credential", value)
        .await
        .map_err(|_| HandlerError::ProviderUnavailable)
}

async fn supabase_rotate(body: &[u8]) -> HandlerResult {
    let map = object(body)?;
    exact_keys(&map, &["mode"])?;
    let mode = required_string(&map, "mode")?;
    if !matches!(mode.as_str(), "service-key" | "db-password") {
        return Err(HandlerError::BadRequest);
    }
    let (provider, url, project_ref, access_token) = supabase_credentials().await?;
    let mut item = provider.read_item(SUPABASE_ITEM).await?;
    let values = item
        .as_object_mut()
        .ok_or(HandlerError::ProviderUnavailable)?;
    let http = client()?;
    let management = format!("https://api.supabase.com/v1/projects/{project_ref}");
    if mode == "db-password" {
        let password = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let (status, _) = response_json(
            http.patch(format!("{management}/database/password"))
                .bearer_auth(&access_token)
                .json(&json!({"password": password})),
        )
        .await?;
        successful(status)?;
        values.insert("database_password".to_string(), Value::String(password));
        write_provider_item(SUPABASE_ITEM, &item).await?;
        return Ok(
            json!({"status": "rotated", "mode": mode, "project_ref": project_ref, "events": [{"step": "supabase.database.password", "status": "rotated"}, {"step": "skarbiec.provider-item", "status": "updated"}]}),
        );
    }
    let old_key = values
        .get("service_role_key")
        .and_then(Value::as_str)
        .map(str::to_string);
    let (create_status, created) = response_json(
        http.post(format!("{management}/api-keys?reveal=true"))
            .bearer_auth(&access_token)
            .json(&json!({"type": "secret", "name": "people-rotator"})),
    )
    .await?;
    successful(create_status)?;
    let new_key = created
        .get("api_key")
        .and_then(Value::as_str)
        .ok_or(HandlerError::UpstreamFailure)?
        .to_string();
    let new_id = created
        .get("id")
        .and_then(Value::as_str)
        .ok_or(HandlerError::UpstreamFailure)?
        .to_string();
    let (verify_status, _) = response_json(
        http.get(format!("{url}/rest/v1/"))
            .header("apikey", &new_key)
            .bearer_auth(&new_key),
    )
    .await?;
    successful(verify_status)?;
    values.insert(
        "service_role_key".to_string(),
        Value::String(new_key.clone()),
    );
    write_provider_item(SUPABASE_ITEM, &item).await?;
    let (list_status, listed) = response_json(
        http.get(format!("{management}/api-keys?reveal=true"))
            .bearer_auth(&access_token),
    )
    .await?;
    successful(list_status)?;
    let mut revoked = Vec::new();
    for key in listed.as_array().into_iter().flatten() {
        let id = key.get("id").and_then(Value::as_str);
        let value = key.get("api_key").and_then(Value::as_str);
        if id.is_some_and(|id| id != new_id)
            && old_key.as_deref().is_some_and(|old| value == Some(old))
        {
            let id = id.expect("checked id");
            let (status, _) = response_json(
                http.delete(format!(
                    "{management}/api-keys/{id}?was_compromised=false&reason=people-rotator"
                ))
                .bearer_auth(&access_token),
            )
            .await?;
            successful(status)?;
            revoked.push(id.to_string());
        }
    }
    Ok(
        json!({"status": "rotated", "mode": mode, "project_ref": project_ref, "key_id": new_id, "events": [{"step": "supabase.api_key.create", "status": "created"}, {"step": "supabase.api_key.verify", "status": "verified"}, {"step": "skarbiec.provider-item", "status": "updated"}, {"step": "supabase.api_key.revoke_old", "status": "completed", "revoked_ids": revoked}]}),
    )
}

async fn weles_enqueue(body: &[u8]) -> HandlerResult {
    let map = object(body)?;
    exact_keys(&map, &["task"])?;
    let task = map
        .get("task")
        .and_then(Value::as_object)
        .ok_or(HandlerError::BadRequest)?;
    let allowed = [
        "url",
        "objective",
        "constraints",
        "env",
        "flow_name",
        "max_steps",
        "headless",
        "idempotency_key",
        "webhook_url",
        "priority",
        "verification_required",
        "verifier",
    ];
    exact_keys(task, &allowed)?;
    required_string(task, "url")?;
    required_string(task, "objective")?;
    required_string(task, "flow_name")?;
    let idempotency_key = required_string(task, "idempotency_key")?;
    let provider = provider_client("people").await?;
    let url = provider_origin(&provider.read_string(WELES_ITEM, "url").await?)?;
    let key = provider.read_string(WELES_ITEM, "service_role_key").await?;
    let table = format!("{url}/rest/v1/account_action_logs");
    let select = "id,status,action,platform,params";
    let idempotency_filter = format!("eq.{idempotency_key}");
    let (lookup_status, existing) = response_json(
        client()?
            .get(&table)
            .header("apikey", &key)
            .bearer_auth(&key)
            .query(&[
                ("select", select),
                ("params->>idempotency_key", idempotency_filter.as_str()),
                ("limit", "1"),
            ]),
    )
    .await?;
    successful(lookup_status)?;
    if let Some(row) = existing.as_array().and_then(|rows| rows.first()) {
        return Ok(json!({"created": false, "run_id": row.get("id"), "status": row.get("status")}));
    }
    let row = json!({"account_id": null, "action": "generic_browser_task", "platform": "generic", "status": "queued", "scheduled_at": chrono::Utc::now().to_rfc3339(), "priority": task.get("priority"), "params": task});
    let (status, inserted) = response_json(
        client()?
            .post(format!("{table}?select={select}"))
            .header("apikey", &key)
            .bearer_auth(&key)
            .header("Prefer", "return=representation")
            .json(&row),
    )
    .await?;
    successful(status)?;
    let result = inserted
        .as_array()
        .and_then(|rows| rows.first())
        .unwrap_or(&inserted);
    Ok(
        json!({"created": true, "run_id": result.get("id"), "status": result.get("status").and_then(Value::as_str).unwrap_or("queued")}),
    )
}

async fn prerequisites(body: &[u8]) -> HandlerResult {
    let map = object(body)?;
    exact_keys(&map, &["actions"])?;
    let actions = strings(&map, "actions")?;
    if actions
        .iter()
        .any(|action| !supports(action) && action != "supabase.credentials.rotate")
    {
        return Err(HandlerError::BadRequest);
    }
    let configured = [GITHUB_ITEM, SLACK_ITEM, SUPABASE_ITEM, WELES_ITEM];
    let policy =
        crate::config::integration_provider("people").ok_or(HandlerError::ProviderUnavailable)?;
    if policy.items().len() != configured.len()
        || configured
            .iter()
            .any(|item| !policy.items().iter().any(|allowed| allowed == item))
    {
        return Err(HandlerError::ProviderUnavailable);
    }
    let provider = crate::skarbiec::Client::integration_provider("people")
        .map_err(|_| HandlerError::ProviderUnavailable)?;
    let mut prerequisites = Vec::new();
    for (name, item) in [
        ("github", GITHUB_ITEM),
        ("slack", SLACK_ITEM),
        ("supabase", SUPABASE_ITEM),
        ("weles", WELES_ITEM),
    ] {
        let required = actions.iter().any(|action| {
            action.starts_with(name)
                || (name == "supabase" && action == "supabase.credentials.rotate")
        });
        if !required {
            continue;
        }
        let required_fields = match name {
            "github"
                if actions
                    .iter()
                    .any(|action| action == "github.org.revoke_fine_grained_pat_grants") =>
            {
                vec!["token", "app_token"]
            }
            "github" => vec!["token"],
            "slack" => vec!["token"],
            "supabase"
                if actions
                    .iter()
                    .any(|action| action == "supabase.credentials.rotate") =>
            {
                vec!["url", "project_ref", "access_token", "service_role_key"]
            }
            "supabase" => vec!["url", "service_role_key"],
            "weles" => vec!["url", "service_role_key"],
            _ => return Err(HandlerError::BadRequest),
        };
        let item_value = provider.read_item(item).await.ok();
        let missing_fields = required_fields
            .iter()
            .filter(|field| {
                item_value
                    .as_ref()
                    .and_then(|value| value.get(**field))
                    .and_then(Value::as_str)
                    .is_none_or(|value| value.trim().is_empty())
            })
            .copied()
            .collect::<Vec<_>>();
        let missing_items = if item_value.is_some() {
            Vec::<String>::new()
        } else {
            vec![item.to_string()]
        };
        let required_access = if name == "supabase"
            && actions
                .iter()
                .any(|action| action == "supabase.credentials.rotate")
        {
            "read-write"
        } else {
            "read"
        };
        prerequisites.push(json!({
            "provider": name,
            "ready": missing_items.is_empty() && missing_fields.is_empty(),
            "required_items": [item],
            "required_access": required_access,
            "required_fields": required_fields,
            "missing_items": missing_items,
            "missing_fields": missing_fields
        }));
    }
    Ok(
        json!({"client": "people-rotator", "actions": actions, "provider_policy_items": configured, "prerequisites": prerequisites}),
    )
}

pub(super) async fn handle(action: &str, body: &[u8]) -> HandlerResult {
    match action {
        "prerequisites" => prerequisites(body).await,
        "github.org.invite_member" | "github.org.remove_member" | "github.membership.check" => {
            github_org_action(action, body).await
        }
        "github.team.add_member" => github_team_add(body).await,
        "github.repo.remove_collaborator" => github_repo_remove(body).await,
        "github.org.revoke_fine_grained_pat_grants" => github_pat_revoke(body).await,
        "github.repo.transfer" => github_transfer(body).await,
        "slack.user.invite" | "slack.user.deactivate" => slack(action, body).await,
        "supabase.auth.invite_user" | "supabase.auth.ban_user" => supabase_auth(action, body).await,
        "supabase.credentials.rotate" => supabase_rotate(body).await,
        "weles.queue.enqueue" => weles_enqueue(body).await,
        _ => Err(HandlerError::BadRequest),
    }
}
