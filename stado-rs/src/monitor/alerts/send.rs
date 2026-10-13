//! Channel delivery functions, split from `alerts/mod.rs` to keep every file
//! under the repository's per-file line budget. Resolution (Skarbiec reads)
//! stays in `alerts/resolve.rs`'s `AlertChannels::from_env`; everything here
//! delivers an already-resolved channel struct: an HTTP request, or for email
//! one `skrzynka message send`.

use serde_json::json;

use super::{MostChannel, PubSubChannel, SendgridChannel, SkrzynkaChannel, TelegramChannel};

/// Error on a non-2xx response, including the upstream body.
async fn ensure_success(response: reqwest::Response) -> Result<(), String> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = crate::primitives::http::refused_body(response).await;
    Err(format!("HTTP {status}: {body}"))
}

pub(super) async fn send_slack(
    client: &reqwest::Client,
    url: &str,
    message: &str,
) -> Result<(), String> {
    let response = crate::wait::request(client.post(url).json(&json!({"text": message})))
        .await
        .map_err(|e| e.to_string())?;
    ensure_success(response).await
}

pub(super) async fn send_telegram(
    client: &reqwest::Client,
    channel: &TelegramChannel,
    message: &str,
) -> Result<(), String> {
    let url = format!("{}/bot{}/sendMessage", channel.api_base, channel.token);
    let response = crate::wait::request(
        client
            .post(url)
            .json(&json!({"chat_id": channel.chat_id, "text": message, "parse_mode": "Markdown"})),
    )
    .await
    .map_err(|e| e.to_string())?;
    ensure_success(response).await
}

pub(super) async fn send_email(
    client: &reqwest::Client,
    channel: &SendgridChannel,
    subject: &str,
    body: &str,
) -> Result<(), String> {
    let response = crate::wait::request(
        client
            .post(&channel.url)
            .bearer_auth(&channel.api_key)
            .json(&json!({
                "personalizations": [{"to": [{"email": channel.to}]}],
                "from": {"email": channel.from},
                "subject": subject,
                "content": [{"type": "text/plain", "value": body}],
            })),
    )
    .await
    .map_err(|e| e.to_string())?;
    ensure_success(response).await
}

/// Email through Skrzynka. The body is staged in a private file because
/// `skrzynka message send` reads it from one, and Skrzynka answers with the
/// outbound row it recorded: only a row in state `sent` is a delivered page;
/// `failed` and `uncertain` carry Skrzynka's own error code and message.
pub(super) async fn send_skrzynka(
    channel: &SkrzynkaChannel,
    subject: &str,
    body: &str,
) -> Result<String, String> {
    use std::io::Write;
    let program = crate::mail::SKRZYNKA;
    let stage =
        |error: std::io::Error| format!("cannot stage the alert body for {program}: {error}");
    let mut staged = tempfile::NamedTempFile::new().map_err(stage)?;
    staged.write_all(body.as_bytes()).map_err(stage)?;
    let output = crate::wait::output_async(
        tokio::process::Command::new(program)
            .arg("--organization")
            .arg(&channel.organization)
            .args(["message", "send", "--mailbox"])
            .arg(&channel.mailbox)
            .arg("--to")
            .arg(&channel.to)
            .arg("--subject")
            .arg(subject)
            .arg("--body-file")
            .arg(staged.path())
            .stdin(std::process::Stdio::null()),
    )
    .await
    .map_err(|error| format!("{program} message send could not run: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{program} message send --mailbox {} exited {}: {}",
            channel.mailbox,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let row: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("{program} message send answered no outbound row: {error}"))?;
    let field = |name: &str| match row.get(name).and_then(serde_json::Value::as_str) {
        Some(value) => value.to_string(),
        None => format!("<no {name}>"),
    };
    match row.get("status").and_then(serde_json::Value::as_str) {
        Some("sent") => Ok(field("id")),
        _ => Err(format!(
            "{program} recorded outbound {} as {}: {}: {}",
            field("id"),
            field("status"),
            field("error_code"),
            field("error_message")
        )),
    }
}

/// Why the channel's mailbox cannot send, read from `skrzynka mailbox list`:
/// Skrzynka does not run here, refuses the organization, holds no mailbox by
/// that id or address, or holds it disabled. Reads only; it sends nothing.
pub(crate) async fn skrzynka_mailbox_problem(
    channel: &SkrzynkaChannel,
) -> Result<(), SkrzynkaMailboxProblem> {
    let program = crate::mail::SKRZYNKA;
    let output = crate::wait::output_async(
        tokio::process::Command::new(program)
            .arg("--organization")
            .arg(&channel.organization)
            .args(["mailbox", "list"])
            .stdin(std::process::Stdio::null()),
    )
    .await
    .map_err(|error| {
        SkrzynkaMailboxProblem::Unasked(format!("{program} mailbox list could not run: {error}"))
    })?;
    if !output.status.success() {
        return Err(SkrzynkaMailboxProblem::Refused(format!(
            "{program} --organization {} mailbox list exited {}: {}",
            channel.organization,
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let mailboxes: Vec<serde_json::Value> =
        serde_json::from_slice(&output.stdout).map_err(|error| {
            SkrzynkaMailboxProblem::Unasked(format!(
                "{program} mailbox list answered no mailbox list: {error}"
            ))
        })?;
    let text = |mailbox: &serde_json::Value, name: &str| {
        mailbox
            .get(name)
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
    };
    let selector = channel.mailbox.as_str();
    let found = mailboxes.iter().find(|mailbox| {
        text(mailbox, "id").as_deref() == Some(selector)
            || text(mailbox, "email").is_some_and(|email| email.eq_ignore_ascii_case(selector))
    });
    match found {
        None => Err(SkrzynkaMailboxProblem::Refused(format!(
            "Skrzynka holds no mailbox {selector} in organization {}",
            channel.organization
        ))),
        Some(mailbox) if mailbox.get("enabled") != Some(&serde_json::Value::Bool(true)) => {
            Err(SkrzynkaMailboxProblem::Refused(format!(
                "Skrzynka holds mailbox {selector} disabled; `{program} --organization {} \
                 mailbox enable <id>` turns it on",
                channel.organization
            )))
        }
        Some(_) => Ok(()),
    }
}

/// A mailbox Skrzynka answered about, apart from a Skrzynka that could not
/// be asked: the first is the deployment's to fix, the second this host's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SkrzynkaMailboxProblem {
    Refused(String),
    Unasked(String),
}

pub(super) async fn send_pubsub(
    client: &reqwest::Client,
    channel: &PubSubChannel,
    message: &str,
) -> Result<(), String> {
    use base64::Engine;
    let data = base64::engine::general_purpose::STANDARD.encode(message);
    let url = format!("{}/v1/{}:publish", channel.base_url, channel.topic);
    let response = crate::wait::request(
        client
            .post(url)
            .bearer_auth(&channel.token)
            .json(&json!({"messages": [{"data": data}]})),
    )
    .await
    .map_err(|e| e.to_string())?;
    ensure_success(response).await
}

/// SMS through the `most` integration domain: the same Twilio Programmable
/// Messaging call the dashboard's `most/send-sms` action makes, but delivered
/// in-process — an alert path that routed through the local dashboard would
/// go quiet in exactly the outage it exists to report.
pub(super) async fn send_most(
    client: &reqwest::Client,
    channel: &MostChannel,
    message: &str,
) -> Result<(), String> {
    let endpoint = format!(
        "{}/{}/Accounts/{}/Messages.json",
        channel.api_base, channel.api_version, channel.account_sid
    );
    let mut form = vec![("To", channel.phone.clone()), ("Body", message.to_string())];
    if let Some(service_sid) = &channel.messaging_service_sid {
        form.push(("MessagingServiceSid", service_sid.clone()));
    } else if let Some(from) = &channel.from_number {
        form.push(("From", from.clone()));
    } else {
        return Err("most channel has neither messaging_service_sid nor from_number".to_string());
    }
    let response = crate::wait::request(
        client
            .post(endpoint)
            .basic_auth(&channel.account_sid, Some(&channel.auth_token))
            .form(&form),
    )
    .await
    .map_err(|e| e.to_string())?;
    ensure_success(response).await
}

/// The alert's subject, or the message's first line when it states none: a
/// line is the message's own unit, where a character count cut words apart.
pub(super) fn email_subject<'a>(subject: &'a str, message: &'a str) -> String {
    if !subject.is_empty() {
        return subject.to_string();
    }
    match message.split_once('\n') {
        Some((first_line, _)) => first_line.to_string(),
        None => message.to_string(),
    }
}
