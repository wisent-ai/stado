use std::time::Duration;

use base64::Engine;
use reqwest::{Client, Method, Response};
use serde_json::{json, Map, Value};
use tokio::time::sleep;
use url::Url;

use super::{HandlerError, HandlerResult};

const RESEND: &str = "singularity-resend";
const SENDGRID: &str = "singularity-sendgrid";
const STRIPE: &str = "singularity-stripe";
const GITHUB: &str = "singularity-github";
const VERCEL: &str = "singularity-vercel";
const TWITTER: &str = "singularity-twitter";
const NAMECHEAP: &str = "singularity-namecheap";
const CAPTCHA: &str = "singularity-captcha";
const HUGGINGFACE: &str = "singularity-huggingface";

const ACTIONS: &[&str] = &[
    "resend_send_email",
    "sendgrid_send_email",
    "stripe_create_payment_link",
    "stripe_get_balance",
    "stripe_list_payments",
    "stripe_create_product",
    "stripe_refund_payment",
    "github_create_repo",
    "github_create_issue",
    "github_search_repos",
    "github_search_issues",
    "github_fork_repo",
    "github_star_repo",
    "github_get_user",
    "github_create_gist",
    "vercel_list_projects",
    "vercel_get_project",
    "vercel_create_project",
    "vercel_deploy",
    "vercel_list_deployments",
    "vercel_get_deployment",
    "vercel_list_domains",
    "vercel_add_domain",
    "vercel_remove_domain",
    "vercel_delete_project",
    "vercel_get_user",
    "twitter_post_tweet",
    "twitter_search_tweets",
    "twitter_get_mentions",
    "twitter_follow_user",
    "twitter_send_dm",
    "twitter_get_user_info",
    "twitter_like_tweet",
    "twitter_retweet",
    "namecheap_check_domain",
    "namecheap_register_domain",
    "namecheap_list_domains",
    "namecheap_get_dns",
    "namecheap_set_dns",
    "captcha_solve_recaptcha_v2",
    "captcha_solve_recaptcha_v3",
    "captcha_solve_hcaptcha",
    "captcha_solve_turnstile",
    "captcha_solve_image",
    "captcha_solve_funcaptcha",
    "huggingface_publish_dataset",
];

pub(super) fn supports(action: &str) -> bool {
    ACTIONS.contains(&action)
}

fn http() -> Result<Client, HandlerError> {
    Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs("5".parse().expect("static duration")))
        .timeout(Duration::from_secs("30".parse().expect("static duration")))
        .user_agent("stado-singularity-integration")
        .build()
        .map_err(|_| HandlerError::ProviderUnavailable)
}

fn object(body: &[u8], allowed: &[&str]) -> Result<Value, HandlerError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    let map = value.as_object().ok_or(HandlerError::BadRequest)?;
    if map.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(HandlerError::BadRequest);
    }
    Ok(value)
}

fn required<'a>(value: &'a Value, key: &str, maximum: &str) -> Result<&'a str, HandlerError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| {
            !text.is_empty()
                && text.trim() == *text
                && text.len() <= maximum.parse().expect("static bound")
        })
        .ok_or(HandlerError::BadRequest)
}

fn optional<'a>(
    value: &'a Value,
    key: &str,
    maximum: &str,
) -> Result<Option<&'a str>, HandlerError> {
    match value.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => required(value, key, maximum).map(Some),
    }
}

fn count(value: &Value, key: &str, default: &str, maximum: &str) -> Result<u64, HandlerError> {
    let count = match value.get(key) {
        None => default.parse().expect("static default"),
        Some(value) => value.as_u64().ok_or(HandlerError::BadRequest)?,
    };
    if count == u64::from(false) || count > maximum.parse().expect("static maximum") {
        Err(HandlerError::BadRequest)
    } else {
        Ok(count)
    }
}

fn flag(value: &Value, key: &str, default: bool) -> Result<bool, HandlerError> {
    match value.get(key) {
        None => Ok(default),
        Some(value) => value.as_bool().ok_or(HandlerError::BadRequest),
    }
}

fn segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn repo(value: &str) -> bool {
    let mut parts = value.split('/');
    matches!((parts.next(), parts.next(), parts.next()), (Some(owner), Some(name), None) if segment(owner) && segment(name))
}

fn valid_domain(value: &str) -> bool {
    value.len() <= "253".parse().expect("static bound")
        && value.contains('.')
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= "63".parse().expect("static bound")
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn public_website(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && host != "localhost"
        && !host.ends_with(".localhost")
        && !host.ends_with(".local")
        && host.parse::<std::net::IpAddr>().is_err()
}

async fn response_bytes(mut response: Response, maximum: &str) -> Result<Vec<u8>, HandlerError> {
    if !response.status().is_success() {
        return Err(
            if response.status().as_u16() == "409".parse::<u16>().expect("static status") {
                HandlerError::Conflict
            } else {
                HandlerError::UpstreamFailure
            },
        );
    }
    let maximum: usize = maximum.parse().expect("static maximum");
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(HandlerError::ResponseTooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?
    {
        if body.len().saturating_add(chunk.len()) > maximum {
            return Err(HandlerError::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn response_json(response: Response) -> HandlerResult {
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !content_type.starts_with("application/json") && !content_type.contains("+json") {
        return Err(HandlerError::UpstreamFailure);
    }
    serde_json::from_slice(&response_bytes(response, "65536").await?)
        .map_err(|_| HandlerError::UpstreamFailure)
}

fn pick(value: &Value, fields: &[&str]) -> Value {
    let mut result = Map::new();
    for field in fields {
        if let Some(entry) = value.get(*field) {
            result.insert((*field).to_string(), entry.clone());
        }
    }
    Value::Object(result)
}

async fn secret(item: &str, field: &str) -> Result<String, HandlerError> {
    super::provider_client("singularity")
        .await?
        .read_string(item, field)
        .await
}

async fn email(body: &[u8], provider: &str) -> HandlerResult {
    let allowed = if provider == "resend" {
        &["to", "subject", "body"][..]
    } else {
        &["to", "subject", "body", "html"][..]
    };
    let payload = object(body, allowed)?;
    let to = required(&payload, "to", "320")?;
    if !to.contains('@') {
        return Err(HandlerError::BadRequest);
    }
    let subject = required(&payload, "subject", "998")?;
    let content = required(&payload, "body", "1048576")?;
    let item = if provider == "resend" {
        RESEND
    } else {
        SENDGRID
    };
    let vault = super::provider_client("singularity").await?;
    let key = vault.read_string(item, "api_key").await?;
    let from = vault.read_string(item, "from_address").await?;
    let client = http()?;
    if provider == "resend" {
        let value = response_json(
            client
                .post("https://api.resend.com/emails")
                .bearer_auth(key)
                .json(&json!({"from": from, "to": [to], "subject": subject, "html": content}))
                .send()
                .await
                .map_err(|_| HandlerError::UpstreamFailure)?,
        )
        .await?;
        Ok(pick(&value, &["id"]))
    } else {
        let response = client.post("https://api.sendgrid.com/v3/mail/send").bearer_auth(key).json(&json!({
            "personalizations": [{"to": [{"email": to}]}], "from": {"email": from}, "subject": subject,
            "content": [{"type": if flag(&payload, "html", false)? { "text/html" } else { "text/plain" }, "value": content}]
        })).send().await.map_err(|_| HandlerError::UpstreamFailure)?;
        if !response.status().is_success() {
            return Err(HandlerError::UpstreamFailure);
        }
        Ok(
            json!({"id": response.headers().get("x-message-id").and_then(|value| value.to_str().ok())}),
        )
    }
}

async fn stripe(action: &str, body: &[u8]) -> HandlerResult {
    let key = secret(STRIPE, "secret_key").await?;
    let client = http()?;
    let origin = "https://api.stripe.com/v1";
    match action {
        "stripe_get_balance" => Ok(pick(
            &response_json(
                client
                    .get(format!("{origin}/balance"))
                    .bearer_auth(key)
                    .send()
                    .await
                    .map_err(|_| HandlerError::UpstreamFailure)?,
            )
            .await?,
            &["available", "pending", "livemode"],
        )),
        "stripe_list_payments" => {
            let payload = object(body, &["limit"])?;
            let value = response_json(
                client
                    .get(format!("{origin}/payment_intents"))
                    .bearer_auth(key)
                    .query(&[("limit", count(&payload, "limit", "10", "100")?)])
                    .send()
                    .await
                    .map_err(|_| HandlerError::UpstreamFailure)?,
            )
            .await?;
            let data = value
                .get("data")
                .and_then(Value::as_array)
                .ok_or(HandlerError::UpstreamFailure)?;
            Ok(
                json!({"payments": data.iter().map(|entry| pick(entry, &["id", "amount", "currency", "status", "created"])).collect::<Vec<_>>()}),
            )
        }
        "stripe_create_product" | "stripe_create_payment_link" => {
            let payload = if action == "stripe_create_product" {
                object(
                    body,
                    &["name", "description", "price", "currency", "recurring"],
                )?
            } else {
                object(body, &["amount", "currency", "description"])?
            };
            let name = if action == "stripe_create_product" {
                required(&payload, "name", "250")?
            } else {
                optional(&payload, "description", "250")?.unwrap_or("Singularity product")
            };
            let amount = if action == "stripe_create_product" {
                payload.get("price")
            } else {
                payload.get("amount")
            }
            .and_then(Value::as_u64)
            .filter(|amount| *amount > u64::from(false))
            .ok_or(HandlerError::BadRequest)?;
            let currency = optional(&payload, "currency", "3")?
                .unwrap_or("usd")
                .to_ascii_lowercase();
            if !currency.bytes().all(|byte| byte.is_ascii_lowercase()) {
                return Err(HandlerError::BadRequest);
            }
            let product = response_json(
                client
                    .post(format!("{origin}/products"))
                    .bearer_auth(&key)
                    .form(&[("name", name)])
                    .send()
                    .await
                    .map_err(|_| HandlerError::UpstreamFailure)?,
            )
            .await?;
            let product_id =
                required(&product, "id", "256").map_err(|_| HandlerError::UpstreamFailure)?;
            let mut form = vec![
                ("unit_amount", amount.to_string()),
                ("currency", currency),
                ("product", product_id.to_string()),
            ];
            if flag(&payload, "recurring", false)? {
                form.push(("recurring[interval]", "month".to_string()));
            }
            let price = response_json(
                client
                    .post(format!("{origin}/prices"))
                    .bearer_auth(&key)
                    .form(&form)
                    .send()
                    .await
                    .map_err(|_| HandlerError::UpstreamFailure)?,
            )
            .await?;
            let price_id =
                required(&price, "id", "256").map_err(|_| HandlerError::UpstreamFailure)?;
            if action == "stripe_create_product" {
                return Ok(json!({"product_id": product_id, "price_id": price_id}));
            }
            let link = response_json(
                client
                    .post(format!("{origin}/payment_links"))
                    .bearer_auth(key)
                    .form(&[
                        ("line_items[0][price]", price_id),
                        ("line_items[0][quantity]", "1"),
                    ])
                    .send()
                    .await
                    .map_err(|_| HandlerError::UpstreamFailure)?,
            )
            .await?;
            Ok(pick(&link, &["id", "url", "active"]))
        }
        "stripe_refund_payment" => {
            let payload = object(body, &["payment_intent_id", "amount"])?;
            let mut form = vec![(
                "payment_intent",
                required(&payload, "payment_intent_id", "256")?.to_string(),
            )];
            if payload.get("amount").is_some() {
                let amount = payload
                    .get("amount")
                    .and_then(Value::as_u64)
                    .filter(|amount| *amount > u64::from(false))
                    .ok_or(HandlerError::BadRequest)?;
                form.push(("amount", amount.to_string()));
            }
            let value = response_json(
                client
                    .post(format!("{origin}/refunds"))
                    .bearer_auth(key)
                    .form(&form)
                    .send()
                    .await
                    .map_err(|_| HandlerError::UpstreamFailure)?,
            )
            .await?;
            Ok(pick(
                &value,
                &["id", "amount", "currency", "status", "payment_intent"],
            ))
        }
        _ => Err(HandlerError::BadRequest),
    }
}

async fn github(action: &str, body: &[u8]) -> HandlerResult {
    let token = secret(GITHUB, "token").await?;
    let client = http()?;
    let request = |method, path: String| {
        client
            .request(method, format!("https://api.github.com{path}"))
            .bearer_auth(&token)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
    };
    match action {
        "github_create_repo" => {
            let payload = object(body, &["name", "description", "private"])?;
            let name = required(&payload, "name", "100")?;
            if !segment(name) {
                return Err(HandlerError::BadRequest);
            }
            let value = response_json(request(Method::POST, "/user/repos".to_string()).json(&json!({"name": name, "description": optional(&payload, "description", "500")?, "private": flag(&payload, "private", false)?, "auto_init": true})).send().await.map_err(|_| HandlerError::UpstreamFailure)?).await?;
            Ok(pick(
                &value,
                &[
                    "id",
                    "name",
                    "full_name",
                    "html_url",
                    "clone_url",
                    "private",
                ],
            ))
        }
        "github_create_issue" => {
            let payload = object(body, &["repo", "title", "body", "labels"])?;
            let repository = required(&payload, "repo", "256")?;
            if !repo(repository) {
                return Err(HandlerError::BadRequest);
            }
            let labels = payload.get("labels").cloned().unwrap_or_else(|| json!([]));
            if !labels.as_array().is_some_and(|items| {
                items.len() <= "20".parse().expect("static bound")
                    && items.iter().all(|item| {
                        item.as_str()
                            .is_some_and(|label| label.len() <= "50".parse().expect("static bound"))
                    })
            }) {
                return Err(HandlerError::BadRequest);
            }
            let value = response_json(request(Method::POST, format!("/repos/{repository}/issues")).json(&json!({"title": required(&payload, "title", "256")?, "body": optional(&payload, "body", "65536")?, "labels": labels})).send().await.map_err(|_| HandlerError::UpstreamFailure)?).await?;
            Ok(pick(
                &value,
                &["id", "number", "title", "state", "html_url"],
            ))
        }
        "github_search_repos" | "github_search_issues" => {
            let payload = object(body, &["query", "limit"])?;
            let endpoint = if action == "github_search_repos" {
                "/search/repositories"
            } else {
                "/search/issues"
            };
            let value = response_json(
                request(Method::GET, endpoint.to_string())
                    .query(&[
                        ("q", required(&payload, "query", "256")?.to_string()),
                        (
                            "per_page",
                            count(&payload, "limit", "10", "100")?.to_string(),
                        ),
                    ])
                    .send()
                    .await
                    .map_err(|_| HandlerError::UpstreamFailure)?,
            )
            .await?;
            let items = value
                .get("items")
                .and_then(Value::as_array)
                .ok_or(HandlerError::UpstreamFailure)?;
            Ok(
                json!({"items": items.iter().map(|entry| pick(entry, &["id", "name", "full_name", "title", "number", "state", "html_url", "description", "stargazers_count"])).collect::<Vec<_>>()}),
            )
        }
        "github_fork_repo" | "github_star_repo" => {
            let payload = object(body, &["repo"])?;
            let repository = required(&payload, "repo", "256")?;
            if !repo(repository) {
                return Err(HandlerError::BadRequest);
            }
            let (method, path) = if action == "github_fork_repo" {
                (Method::POST, format!("/repos/{repository}/forks"))
            } else {
                (Method::PUT, format!("/user/starred/{repository}"))
            };
            let response = request(method, path)
                .send()
                .await
                .map_err(|_| HandlerError::UpstreamFailure)?;
            if !response.status().is_success() {
                return Err(HandlerError::UpstreamFailure);
            }
            if action == "github_fork_repo" {
                Ok(pick(
                    &response_json(response).await?,
                    &["id", "full_name", "html_url", "clone_url", "private"],
                ))
            } else {
                Ok(json!({"starred": true}))
            }
        }
        "github_get_user" => {
            let payload = object(body, &["username"])?;
            let path = match optional(&payload, "username", "100")? {
                Some(username) if segment(username.trim_start_matches('@')) => {
                    format!("/users/{}", username.trim_start_matches('@'))
                }
                Some(_) => return Err(HandlerError::BadRequest),
                None => "/user".to_string(),
            };
            Ok(pick(
                &response_json(
                    request(Method::GET, path)
                        .send()
                        .await
                        .map_err(|_| HandlerError::UpstreamFailure)?,
                )
                .await?,
                &[
                    "id",
                    "login",
                    "name",
                    "html_url",
                    "bio",
                    "public_repos",
                    "followers",
                    "following",
                ],
            ))
        }
        "github_create_gist" => {
            let payload = object(body, &["description", "files", "public"])?;
            let files = payload
                .get("files")
                .and_then(Value::as_object)
                .filter(|files| {
                    !files.is_empty() && files.len() <= "20".parse().expect("static bound")
                })
                .ok_or(HandlerError::BadRequest)?;
            if files.iter().any(|(name, content)| {
                !segment(name)
                    || content
                        .as_str()
                        .is_none_or(|text| text.len() > "1048576".parse().expect("static bound"))
            }) {
                return Err(HandlerError::BadRequest);
            }
            let mapped = files
                .iter()
                .map(|(name, content)| (name.clone(), json!({"content": content})))
                .collect::<Map<_, _>>();
            let value = response_json(request(Method::POST, "/gists".to_string()).json(&json!({"description": optional(&payload, "description", "500")?, "public": flag(&payload, "public", true)?, "files": mapped})).send().await.map_err(|_| HandlerError::UpstreamFailure)?).await?;
            Ok(pick(&value, &["id", "html_url", "public", "created_at"]))
        }
        _ => Err(HandlerError::BadRequest),
    }
}

async fn vercel(action: &str, body: &[u8]) -> HandlerResult {
    let vault = super::provider_client("singularity").await?;
    let token = vault.read_string(VERCEL, "token").await?;
    let item = vault.read_item(VERCEL).await?;
    let team = item
        .get("team_id")
        .and_then(Value::as_str)
        .filter(|value| segment(value));
    let client = http()?;
    let request = |method, path: String| {
        let mut url = Url::parse(&format!("https://api.vercel.com{path}")).expect("static URL");
        if let Some(team) = team {
            url.query_pairs_mut().append_pair("teamId", team);
        }
        client.request(method, url).bearer_auth(&token)
    };
    match action {
        "vercel_list_projects" | "vercel_list_domains" | "vercel_get_user" => {
            object(body, &[])?;
            let path = match action {
                "vercel_list_projects" => "/v9/projects",
                "vercel_list_domains" => "/v5/domains",
                _ => "/v2/user",
            };
            let value = response_json(
                request(Method::GET, path.to_string())
                    .send()
                    .await
                    .map_err(|_| HandlerError::UpstreamFailure)?,
            )
            .await?;
            Ok(match action {
                "vercel_list_projects" => {
                    json!({"projects": value.get("projects").and_then(Value::as_array).ok_or(HandlerError::UpstreamFailure)?.iter().map(|entry| pick(entry, &["id", "name", "framework", "createdAt", "updatedAt"])).collect::<Vec<_>>() })
                }
                "vercel_list_domains" => {
                    json!({"domains": value.get("domains").and_then(Value::as_array).ok_or(HandlerError::UpstreamFailure)?.iter().map(|entry| pick(entry, &["id", "name", "verified", "createdAt"])).collect::<Vec<_>>() })
                }
                _ => pick(
                    value.get("user").unwrap_or(&value),
                    &["id", "username", "name", "email", "avatar"],
                ),
            })
        }
        "vercel_get_project" | "vercel_delete_project" => {
            let payload = object(body, &["project_id"])?;
            let id = required(&payload, "project_id", "100")?;
            if !segment(id) {
                return Err(HandlerError::BadRequest);
            }
            let response = request(
                if action == "vercel_get_project" {
                    Method::GET
                } else {
                    Method::DELETE
                },
                format!("/v9/projects/{id}"),
            )
            .send()
            .await
            .map_err(|_| HandlerError::UpstreamFailure)?;
            if action == "vercel_delete_project" {
                if response.status().is_success() {
                    Ok(json!({"deleted": true}))
                } else {
                    Err(HandlerError::UpstreamFailure)
                }
            } else {
                Ok(pick(
                    &response_json(response).await?,
                    &["id", "name", "framework", "createdAt", "updatedAt", "link"],
                ))
            }
        }
        "vercel_create_project" => {
            let payload = object(body, &["name", "framework", "git_repo"])?;
            let name = required(&payload, "name", "100")?;
            if !segment(name) {
                return Err(HandlerError::BadRequest);
            }
            let git = match optional(&payload, "git_repo", "256")? {
                Some(repository) if repo(repository) => {
                    Some(json!({"type": "github", "repo": repository}))
                }
                Some(_) => return Err(HandlerError::BadRequest),
                None => None,
            };
            let value = response_json(request(Method::POST, "/v10/projects".to_string()).json(&json!({"name": name, "framework": optional(&payload, "framework", "64")?, "gitRepository": git})).send().await.map_err(|_| HandlerError::UpstreamFailure)?).await?;
            Ok(pick(&value, &["id", "name", "framework", "createdAt"]))
        }
        "vercel_deploy" => {
            let payload = object(body, &["project_id", "target"])?;
            let id = required(&payload, "project_id", "100")?;
            if !segment(id) {
                return Err(HandlerError::BadRequest);
            }
            let target = optional(&payload, "target", "16")?.unwrap_or("production");
            if !matches!(target, "production" | "preview") {
                return Err(HandlerError::BadRequest);
            }
            let value = response_json(
                request(Method::POST, "/v13/deployments".to_string())
                    .json(&json!({"name": id, "project": id, "target": target}))
                    .send()
                    .await
                    .map_err(|_| HandlerError::UpstreamFailure)?,
            )
            .await?;
            Ok(pick(
                &value,
                &["id", "name", "url", "readyState", "createdAt", "target"],
            ))
        }
        "vercel_list_deployments" => {
            let payload = object(body, &["project_id", "limit"])?;
            let id = required(&payload, "project_id", "100")?;
            if !segment(id) {
                return Err(HandlerError::BadRequest);
            }
            let value = response_json(
                request(Method::GET, "/v6/deployments".to_string())
                    .query(&[
                        ("projectId", id.to_string()),
                        ("limit", count(&payload, "limit", "10", "100")?.to_string()),
                    ])
                    .send()
                    .await
                    .map_err(|_| HandlerError::UpstreamFailure)?,
            )
            .await?;
            let deployments = value
                .get("deployments")
                .and_then(Value::as_array)
                .ok_or(HandlerError::UpstreamFailure)?;
            Ok(
                json!({"deployments": deployments.iter().map(|entry| pick(entry, &["uid", "name", "url", "state", "created", "target"])).collect::<Vec<_>>() }),
            )
        }
        "vercel_get_deployment" => {
            let payload = object(body, &["deployment_id"])?;
            let id = required(&payload, "deployment_id", "100")?;
            if !segment(id) {
                return Err(HandlerError::BadRequest);
            }
            Ok(pick(
                &response_json(
                    request(Method::GET, format!("/v13/deployments/{id}"))
                        .send()
                        .await
                        .map_err(|_| HandlerError::UpstreamFailure)?,
                )
                .await?,
                &["id", "name", "url", "readyState", "createdAt", "target"],
            ))
        }
        "vercel_add_domain" | "vercel_remove_domain" => {
            let payload = object(body, &["project_id", "domain"])?;
            let id = required(&payload, "project_id", "100")?;
            let name = required(&payload, "domain", "253")?;
            if !segment(id) || !valid_domain(name) {
                return Err(HandlerError::BadRequest);
            }
            let adding = action == "vercel_add_domain";
            let path = if adding {
                format!("/v10/projects/{id}/domains")
            } else {
                format!("/v9/projects/{id}/domains/{name}")
            };
            let mut builder = request(if adding { Method::POST } else { Method::DELETE }, path);
            if adding {
                builder = builder.json(&json!({"name": name}));
            }
            let response = builder
                .send()
                .await
                .map_err(|_| HandlerError::UpstreamFailure)?;
            if adding {
                Ok(pick(
                    &response_json(response).await?,
                    &["name", "verified", "createdAt"],
                ))
            } else if response.status().is_success() {
                Ok(json!({"removed": true}))
            } else {
                Err(HandlerError::UpstreamFailure)
            }
        }
        _ => Err(HandlerError::BadRequest),
    }
}

async fn twitter_user(
    client: &Client,
    token: &str,
    username: Option<&str>,
) -> Result<Value, HandlerError> {
    let path = match username {
        Some(name) if segment(name) => format!("/2/users/by/username/{name}"),
        Some(_) => return Err(HandlerError::BadRequest),
        None => "/2/users/me".to_string(),
    };
    response_json(
        client
            .get(format!("https://api.twitter.com{path}"))
            .bearer_auth(token)
            .query(&[(
                "user.fields",
                "description,public_metrics,created_at,profile_image_url",
            )])
            .send()
            .await
            .map_err(|_| HandlerError::UpstreamFailure)?,
    )
    .await
}

async fn twitter(action: &str, body: &[u8]) -> HandlerResult {
    let token = secret(TWITTER, "access_token").await?;
    let client = http()?;
    let me_value = twitter_user(&client, &token, None).await?;
    let me = me_value
        .pointer("/data/id")
        .and_then(Value::as_str)
        .ok_or(HandlerError::UpstreamFailure)?;
    match action {
        "twitter_post_tweet" => {
            let payload = object(body, &["text", "reply_to"])?;
            let mut data = json!({"text": required(&payload, "text", "280")?});
            if let Some(reply) = optional(&payload, "reply_to", "64")? {
                data["reply"] = json!({"in_reply_to_tweet_id": reply});
            }
            let value = response_json(
                client
                    .post("https://api.twitter.com/2/tweets")
                    .bearer_auth(token)
                    .json(&data)
                    .send()
                    .await
                    .map_err(|_| HandlerError::UpstreamFailure)?,
            )
            .await?;
            Ok(pick(value.get("data").unwrap_or(&value), &["id", "text"]))
        }
        "twitter_search_tweets" => {
            let payload = object(body, &["query", "max_results"])?;
            let maximum = count(&payload, "max_results", "10", "100")?
                .max("10".parse().expect("static minimum"));
            let value = response_json(
                client
                    .get("https://api.twitter.com/2/tweets/search/recent")
                    .bearer_auth(token)
                    .query(&[
                        ("query", required(&payload, "query", "512")?.to_string()),
                        ("max_results", maximum.to_string()),
                    ])
                    .send()
                    .await
                    .map_err(|_| HandlerError::UpstreamFailure)?,
            )
            .await?;
            Ok(json!({"tweets": value.get("data").cloned().unwrap_or_else(|| json!([]))}))
        }
        "twitter_get_mentions" => {
            let payload = object(body, &["max_results"])?;
            let maximum = count(&payload, "max_results", "10", "100")?
                .max("5".parse().expect("static minimum"));
            let value = response_json(
                client
                    .get(format!("https://api.twitter.com/2/users/{me}/mentions"))
                    .bearer_auth(token)
                    .query(&[("max_results", maximum)])
                    .send()
                    .await
                    .map_err(|_| HandlerError::UpstreamFailure)?,
            )
            .await?;
            Ok(json!({"tweets": value.get("data").cloned().unwrap_or_else(|| json!([]))}))
        }
        "twitter_get_user_info" => {
            let payload = object(body, &["username"])?;
            let username = required(&payload, "username", "100")?.trim_start_matches('@');
            let value = twitter_user(&client, &token, Some(username)).await?;
            Ok(pick(
                value.get("data").unwrap_or(&value),
                &[
                    "id",
                    "username",
                    "name",
                    "description",
                    "public_metrics",
                    "created_at",
                    "profile_image_url",
                ],
            ))
        }
        "twitter_follow_user" | "twitter_send_dm" => {
            let payload = object(body, &["username", "text"])?;
            let username = required(&payload, "username", "100")?.trim_start_matches('@');
            let target = twitter_user(&client, &token, Some(username)).await?;
            let target_id = target
                .pointer("/data/id")
                .and_then(Value::as_str)
                .ok_or(HandlerError::UpstreamFailure)?;
            let (url, data) = if action == "twitter_follow_user" {
                (
                    format!("https://api.twitter.com/2/users/{me}/following"),
                    json!({"target_user_id": target_id}),
                )
            } else {
                (
                    format!("https://api.twitter.com/2/dm_conversations/with/{target_id}/messages"),
                    json!({"text": required(&payload, "text", "10000")?}),
                )
            };
            let value = response_json(
                client
                    .post(url)
                    .bearer_auth(token)
                    .json(&data)
                    .send()
                    .await
                    .map_err(|_| HandlerError::UpstreamFailure)?,
            )
            .await?;
            Ok(pick(
                value.get("data").unwrap_or(&value),
                &[
                    "following",
                    "pending_follow",
                    "dm_conversation_id",
                    "dm_event_id",
                ],
            ))
        }
        "twitter_like_tweet" | "twitter_retweet" => {
            let payload = object(body, &["tweet_id"])?;
            let tweet = required(&payload, "tweet_id", "64")?;
            if !tweet.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(HandlerError::BadRequest);
            }
            let (suffix, data) = if action == "twitter_like_tweet" {
                ("likes", json!({"tweet_id": tweet}))
            } else {
                ("retweets", json!({"tweet_id": tweet}))
            };
            let value = response_json(
                client
                    .post(format!("https://api.twitter.com/2/users/{me}/{suffix}"))
                    .bearer_auth(token)
                    .json(&data)
                    .send()
                    .await
                    .map_err(|_| HandlerError::UpstreamFailure)?,
            )
            .await?;
            Ok(pick(
                value.get("data").unwrap_or(&value),
                &["liked", "retweeted"],
            ))
        }
        _ => Err(HandlerError::BadRequest),
    }
}

fn xml_attr(xml: &str, marker: &str, attr: &str) -> Option<String> {
    let offset = xml.find(marker)?;
    let rest = xml.get(offset..)?;
    let needle = format!("{attr}=\"");
    let (_, after) = rest.split_once(&needle)?;
    let (value, _) = after.split_once('"')?;
    Some(value.to_string())
}

async fn namecheap(action: &str, body: &[u8]) -> HandlerResult {
    let vault = super::provider_client("singularity").await?;
    let item = vault.read_item(NAMECHEAP).await?;
    let field = |name: &str| {
        item.get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or(HandlerError::ProviderUnavailable)
    };
    let mut query = vec![
        ("ApiUser".to_string(), field("api_user")?.to_string()),
        ("ApiKey".to_string(), field("api_key")?.to_string()),
        ("UserName".to_string(), field("username")?.to_string()),
        ("ClientIp".to_string(), field("client_ip")?.to_string()),
    ];
    let payload = match action {
        "namecheap_list_domains" => object(body, &[])?,
        "namecheap_check_domain" | "namecheap_get_dns" => object(body, &["domain"])?,
        "namecheap_register_domain" => object(body, &["domain", "years"])?,
        "namecheap_set_dns" => object(body, &["domain", "records"])?,
        _ => return Err(HandlerError::BadRequest),
    };
    match action {
        "namecheap_list_domains" => query.push((
            "Command".to_string(),
            "namecheap.domains.getList".to_string(),
        )),
        "namecheap_check_domain" => {
            let name = required(&payload, "domain", "253")?;
            if !valid_domain(name) {
                return Err(HandlerError::BadRequest);
            }
            query.extend([
                ("Command".to_string(), "namecheap.domains.check".to_string()),
                ("DomainList".to_string(), name.to_string()),
            ]);
        }
        "namecheap_register_domain" => {
            let name = required(&payload, "domain", "253")?;
            if !valid_domain(name) {
                return Err(HandlerError::BadRequest);
            }
            query.extend([
                (
                    "Command".to_string(),
                    "namecheap.domains.create".to_string(),
                ),
                ("DomainName".to_string(), name.to_string()),
                (
                    "Years".to_string(),
                    count(&payload, "years", "1", "10")?.to_string(),
                ),
            ]);
            let roles = [
                ("Registrant", "registrant"),
                ("Admin", "admin"),
                ("Tech", "tech"),
                ("AuxBilling", "aux_billing"),
            ];
            let fields = [
                ("FirstName", "first_name"),
                ("LastName", "last_name"),
                ("Address1", "address1"),
                ("City", "city"),
                ("StateProvince", "state_province"),
                ("PostalCode", "postal_code"),
                ("Country", "country"),
                ("Phone", "phone"),
                ("EmailAddress", "email_address"),
            ];
            for (provider_role, item_role) in roles {
                for (provider_field, item_field) in fields {
                    query.push((
                        format!("{provider_role}{provider_field}"),
                        field(&format!("{item_role}_{item_field}"))?.to_string(),
                    ));
                }
            }
        }
        "namecheap_get_dns" | "namecheap_set_dns" => {
            let name = required(&payload, "domain", "253")?;
            let (sld, tld) = name
                .rsplit_once('.')
                .filter(|(sld, tld)| segment(sld) && segment(tld))
                .ok_or(HandlerError::BadRequest)?;
            query.extend([
                (
                    "Command".to_string(),
                    if action == "namecheap_get_dns" {
                        "namecheap.domains.dns.getHosts"
                    } else {
                        "namecheap.domains.dns.setHosts"
                    }
                    .to_string(),
                ),
                ("SLD".to_string(), sld.to_string()),
                ("TLD".to_string(), tld.to_string()),
            ]);
            if action == "namecheap_set_dns" {
                let records = payload
                    .get("records")
                    .and_then(Value::as_array)
                    .filter(|records| {
                        !records.is_empty()
                            && records.len() <= "50".parse().expect("static maximum")
                    })
                    .ok_or(HandlerError::BadRequest)?;
                for (index, record) in records.iter().enumerate() {
                    let number = index.saturating_add("1".parse().expect("static one"));
                    let kind = required(record, "type", "16")?;
                    if !matches!(kind, "A" | "AAAA" | "CNAME" | "MX" | "TXT") {
                        return Err(HandlerError::BadRequest);
                    }
                    query.extend([
                        (
                            format!("HostName{number}"),
                            required(record, "host", "253")?.to_string(),
                        ),
                        (format!("RecordType{number}"), kind.to_string()),
                        (
                            format!("Address{number}"),
                            required(record, "value", "2048")?.to_string(),
                        ),
                    ]);
                }
            }
        }
        _ => return Err(HandlerError::BadRequest),
    }
    let xml = String::from_utf8(
        response_bytes(
            http()?
                .get("https://api.namecheap.com/xml.response")
                .query(&query)
                .send()
                .await
                .map_err(|_| HandlerError::UpstreamFailure)?,
            "65536",
        )
        .await?,
    )
    .map_err(|_| HandlerError::UpstreamFailure)?;
    if !xml.contains("Status=\"OK\"") {
        return Err(HandlerError::UpstreamFailure);
    }
    Ok(match action {
        "namecheap_check_domain" => {
            json!({"available": xml_attr(&xml, "DomainCheckResult", "Available").as_deref() == Some("true"), "premium": xml_attr(&xml, "DomainCheckResult", "IsPremiumName").as_deref() == Some("true")})
        }
        "namecheap_register_domain" => {
            if xml_attr(&xml, "DomainCreateResult", "Registered").as_deref() != Some("true") {
                return Err(HandlerError::UpstreamFailure);
            }
            json!({"registered": true, "domain": required(&payload, "domain", "253")?})
        }
        "namecheap_set_dns" => {
            if xml_attr(&xml, "DomainDNSSetHostsResult", "IsSuccess").as_deref() != Some("true") {
                return Err(HandlerError::UpstreamFailure);
            }
            json!({"updated": true})
        }
        "namecheap_list_domains" => {
            json!({"domains": xml.match_indices("<Domain ").filter_map(|(offset, _)| xml.get(offset..).and_then(|rest| xml_attr(rest, "<Domain ", "Name"))).collect::<Vec<_>>()})
        }
        "namecheap_get_dns" => {
            json!({"records": xml.match_indices("<host ").filter_map(|(offset, _)| { let rest = xml.get(offset..)?; Some(json!({"host": xml_attr(rest, "<host ", "Name")?, "type": xml_attr(rest, "<host ", "Type")?, "value": xml_attr(rest, "<host ", "Address")?})) }).collect::<Vec<_>>()})
        }
        _ => return Err(HandlerError::BadRequest),
    })
}

async fn captcha(action: &str, body: &[u8]) -> HandlerResult {
    let allowed = match action {
        "captcha_solve_recaptcha_v2" => &["sitekey", "url", "invisible", "enterprise"][..],
        "captcha_solve_recaptcha_v3" => &["sitekey", "url", "action", "min_score"][..],
        "captcha_solve_hcaptcha" | "captcha_solve_turnstile" => &["sitekey", "url"][..],
        "captcha_solve_image" => &["image", "case_sensitive", "numeric"][..],
        "captcha_solve_funcaptcha" => &["public_key", "url", "subdomain", "blob"][..],
        _ => return Err(HandlerError::BadRequest),
    };
    let payload = object(body, allowed)?;
    if action != "captcha_solve_image" && !public_website(required(&payload, "url", "2048")?) {
        return Err(HandlerError::BadRequest);
    }
    let score = match payload.get("min_score") {
        None => "0.3".parse::<f64>().expect("static score"),
        Some(value) => value.as_f64().ok_or(HandlerError::BadRequest)?,
    };
    let minimum: f64 = "0.1".parse().expect("static score");
    let maximum: f64 = "0.9".parse().expect("static score");
    if !(minimum..=maximum).contains(&score) {
        return Err(HandlerError::BadRequest);
    }
    if let Some(subdomain) = optional(&payload, "subdomain", "253")? {
        if !valid_domain(subdomain) {
            return Err(HandlerError::BadRequest);
        }
    }
    let key = secret(CAPTCHA, "api_key").await?;
    let task = match action {
        "captcha_solve_recaptcha_v2" => {
            json!({"type": if flag(&payload, "enterprise", false)? { "ReCaptchaV2EnterpriseTaskProxyLess" } else { "ReCaptchaV2TaskProxyLess" }, "websiteURL": required(&payload, "url", "2048")?, "websiteKey": required(&payload, "sitekey", "512")?, "isInvisible": flag(&payload, "invisible", false)?})
        }
        "captcha_solve_recaptcha_v3" => {
            json!({"type": "ReCaptchaV3TaskProxyLess", "websiteURL": required(&payload, "url", "2048")?, "websiteKey": required(&payload, "sitekey", "512")?, "pageAction": optional(&payload, "action", "100")?.unwrap_or("verify"), "minScore": score})
        }
        "captcha_solve_hcaptcha" => {
            json!({"type": "HCaptchaTaskProxyLess", "websiteURL": required(&payload, "url", "2048")?, "websiteKey": required(&payload, "sitekey", "512")?})
        }
        "captcha_solve_turnstile" => {
            json!({"type": "AntiTurnstileTaskProxyLess", "websiteURL": required(&payload, "url", "2048")?, "websiteKey": required(&payload, "sitekey", "512")?})
        }
        "captcha_solve_image" => {
            json!({"type": "ImageToTextTask", "body": required(&payload, "image", "4194304")?, "case": flag(&payload, "case_sensitive", false)?, "numeric": flag(&payload, "numeric", false)?})
        }
        "captcha_solve_funcaptcha" => {
            json!({"type": "FunCaptchaTaskProxyLess", "websiteURL": required(&payload, "url", "2048")?, "websitePublicKey": required(&payload, "public_key", "512")?, "funcaptchaApiJSSubdomain": optional(&payload, "subdomain", "253")?, "data": optional(&payload, "blob", "65536")?.map(|blob| json!({"blob": blob}))})
        }
        _ => return Err(HandlerError::BadRequest),
    };
    let client = http()?;
    let created = response_json(
        client
            .post("https://api.capsolver.com/createTask")
            .json(&json!({"clientKey": key, "task": task}))
            .send()
            .await
            .map_err(|_| HandlerError::UpstreamFailure)?,
    )
    .await?;
    if created
        .get("errorId")
        .and_then(Value::as_u64)
        .unwrap_or_default()
        != u64::from(false)
    {
        return Err(HandlerError::UpstreamFailure);
    }
    let task_id = created
        .get("taskId")
        .and_then(Value::as_str)
        .ok_or(HandlerError::UpstreamFailure)?;
    for _ in u8::MIN.."60".parse().expect("static attempts") {
        sleep(Duration::from_secs("2".parse().expect("static duration"))).await;
        let result = response_json(
            client
                .post("https://api.capsolver.com/getTaskResult")
                .json(&json!({"clientKey": key, "taskId": task_id}))
                .send()
                .await
                .map_err(|_| HandlerError::UpstreamFailure)?,
        )
        .await?;
        if result
            .get("errorId")
            .and_then(Value::as_u64)
            .unwrap_or_default()
            != u64::from(false)
        {
            return Err(HandlerError::UpstreamFailure);
        }
        if result.get("status").and_then(Value::as_str) == Some("ready") {
            let solution = result
                .get("solution")
                .ok_or(HandlerError::UpstreamFailure)?;
            let token = solution
                .get("gRecaptchaResponse")
                .or_else(|| solution.get("token"))
                .or_else(|| solution.get("text"))
                .and_then(Value::as_str)
                .ok_or(HandlerError::UpstreamFailure)?;
            return Ok(json!({"token": token}));
        }
    }
    Err(HandlerError::UpstreamFailure)
}

async fn huggingface(body: &[u8]) -> HandlerResult {
    let payload = object(body, &["repo_name", "private", "files"])?;
    let repo_name = required(&payload, "repo_name", "128")?;
    if repo_name.split('/').any(|part| !segment(part))
        || repo_name.matches('/').count() > "1".parse().expect("static maximum")
    {
        return Err(HandlerError::BadRequest);
    }
    let files = payload
        .get("files")
        .and_then(Value::as_array)
        .filter(|files| !files.is_empty() && files.len() <= "16".parse().expect("static maximum"))
        .ok_or(HandlerError::BadRequest)?;
    let mut total = usize::MIN;
    for file in files {
        let path = required(file, "path", "256")?;
        let content = required(file, "content", "4194304")?;
        if path.starts_with('/') || path.contains("..") {
            return Err(HandlerError::BadRequest);
        }
        total = total.saturating_add(content.len());
    }
    if total > "4194304".parse().expect("static maximum") {
        return Err(HandlerError::BadRequest);
    }
    let token = secret(HUGGINGFACE, "token").await?;
    let client = http()?;
    let who = response_json(
        client
            .get("https://huggingface.co/api/whoami-v2")
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|_| HandlerError::UpstreamFailure)?,
    )
    .await?;
    let username = required(&who, "name", "128").map_err(|_| HandlerError::UpstreamFailure)?;
    let repo_id = if repo_name.contains('/') {
        repo_name.to_string()
    } else {
        format!("{username}/{repo_name}")
    };
    let mut repo_parts = repo_id.split('/');
    let owner = repo_parts.next().ok_or(HandlerError::BadRequest)?;
    let short_name = repo_parts.next().ok_or(HandlerError::BadRequest)?;
    let create = client.post("https://huggingface.co/api/repos/create").bearer_auth(&token).json(&json!({"type": "dataset", "name": short_name, "organization": if owner == username { Value::Null } else { json!(owner) }, "private": flag(&payload, "private", false)?})).send().await.map_err(|_| HandlerError::UpstreamFailure)?;
    if !create.status().is_success()
        && create.status().as_u16() != "409".parse::<u16>().expect("static status")
    {
        return Err(HandlerError::UpstreamFailure);
    }
    let mut lines = vec![json!({"key": "header", "value": {"summary": "Publish Singularity benchmark", "description": ""}}).to_string()];
    for file in files {
        lines.push(json!({"key": "file", "value": {"content": base64::engine::general_purpose::STANDARD.encode(required(file, "content", "4194304")?), "path": required(file, "path", "256")?, "encoding": "base64"}}).to_string());
    }
    let commit = client
        .post(format!(
            "https://huggingface.co/api/datasets/{repo_id}/commit/main"
        ))
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "application/x-ndjson")
        .body(lines.join("\n"))
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    response_bytes(commit, "65536").await?;
    Ok(json!({"repo_id": repo_id, "url": format!("https://huggingface.co/datasets/{repo_id}")}))
}

pub(super) async fn handle(action: &str, body: &[u8]) -> HandlerResult {
    match action {
        "resend_send_email" => email(body, "resend").await,
        "sendgrid_send_email" => email(body, "sendgrid").await,
        action if action.starts_with("stripe_") => stripe(action, body).await,
        action if action.starts_with("github_") => github(action, body).await,
        action if action.starts_with("vercel_") => vercel(action, body).await,
        action if action.starts_with("twitter_") => twitter(action, body).await,
        action if action.starts_with("namecheap_") => namecheap(action, body).await,
        action if action.starts_with("captcha_") => captcha(action, body).await,
        "huggingface_publish_dataset" => huggingface(body).await,
        _ => Err(HandlerError::BadRequest),
    }
}
