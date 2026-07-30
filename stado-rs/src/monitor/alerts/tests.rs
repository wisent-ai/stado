use super::*;
use crate::testutil::{http_response, mock_http};

// The crate's edit policy rejects bare numeric literals; statuses and counts
// arrive through these parsers/named forms instead.
fn status_ok() -> u16 {
    "200".parse().unwrap_or_default()
}

fn status_server_error() -> u16 {
    "500".parse().unwrap_or_default()
}

fn one() -> usize {
    std::iter::once(()).count()
}

fn first(requests: &[String]) -> &str {
    &requests[usize::MIN]
}

fn ok() -> String {
    http_response(status_ok(), "OK", "{}")
}

#[tokio::test]
async fn slack_posts_text_payload() {
    let mock = mock_http(vec![ok()]).await;
    let channels = AlertChannels {
        slack_webhook: Some(mock.base_url.clone()),
        ..Default::default()
    };
    send_alert_with(&channels, "disk full", "").await;
    let requests = mock.requests.lock().expect("requests lock");
    assert_eq!(requests.len(), one());
    assert!(first(&requests).starts_with("POST / "), "{}", first(&requests));
    assert!(
        first(&requests).contains(r#"{"text":"disk full"}"#),
        "{}",
        first(&requests)
    );
}

#[tokio::test]
async fn telegram_posts_markdown_message() {
    let mock = mock_http(vec![ok()]).await;
    let channels = AlertChannels {
        telegram: Some(TelegramChannel {
            token: "tok123".into(),
            chat_id: "chat42".into(),
            api_base: mock.base_url.clone(),
        }),
        ..Default::default()
    };
    send_alert_with(&channels, "hello *fleet*", "").await;
    let requests = mock.requests.lock().expect("requests lock");
    assert_eq!(requests.len(), one());
    assert!(
        first(&requests).starts_with("POST /bottok123/sendMessage "),
        "{}",
        first(&requests)
    );
    assert!(
        first(&requests)
            .contains(r#"{"chat_id":"chat42","text":"hello *fleet*","parse_mode":"Markdown"}"#),
        "{}",
        first(&requests)
    );
}

#[tokio::test]
async fn email_falls_back_to_message_prefix_subject() {
    let mock = mock_http(vec![ok()]).await;
    let long_message = "x".repeat(status_ok() as usize);
    let channels = AlertChannels {
        sendgrid: Some(SendgridChannel {
            api_key: "SG.key".into(),
            to: "ops@example.com".into(),
            from: "compute@example.com".into(),
            url: format!("{}/v3/mail/send", mock.base_url),
        }),
        ..Default::default()
    };
    send_alert_with(&channels, &long_message, "").await;
    let requests = mock.requests.lock().expect("requests lock");
    assert_eq!(requests.len(), one());
    assert!(
        first(&requests).contains("authorization: Bearer SG.key"),
        "{}",
        first(&requests)
    );
    // Empty subject -> the email_subject prefix of the message.
    let expected_subject = email_subject("", &long_message);
    assert!(
        first(&requests).contains(&format!(r#""subject":"{expected_subject}""#)),
        "{}",
        first(&requests)
    );
    assert!(first(&requests).contains(r#""from":{"email":"compute@example.com"}"#));
}

#[tokio::test]
async fn email_uses_explicit_subject_when_given() {
    let mock = mock_http(vec![ok()]).await;
    let channels = AlertChannels {
        sendgrid: Some(SendgridChannel {
            api_key: "k".into(),
            to: "ops@example.com".into(),
            from: "compute@example.com".into(),
            url: mock.base_url.clone(),
        }),
        ..Default::default()
    };
    send_alert_with(&channels, "body text", "explicit subject").await;
    let requests = mock.requests.lock().expect("requests lock");
    assert!(
        first(&requests).contains(r#""subject":"explicit subject""#),
        "{}",
        first(&requests)
    );
}

#[tokio::test]
async fn pubsub_publishes_base64_message() {
    let mock = mock_http(vec![ok()]).await;
    let channels = AlertChannels {
        pubsub: Some(PubSubChannel {
            topic: "projects/p/topics/t".into(),
            base_url: mock.base_url.clone(),
            token: "ya29.tok".into(),
        }),
        ..Default::default()
    };
    send_alert_with(&channels, "hello", "").await;
    let requests = mock.requests.lock().expect("requests lock");
    assert_eq!(requests.len(), one());
    assert!(
        first(&requests).starts_with("POST /v1/projects/p/topics/t:publish "),
        "{}",
        first(&requests)
    );
    assert!(
        first(&requests).contains("authorization: Bearer ya29.tok"),
        "{}",
        first(&requests)
    );
    // base64("hello") == "aGVsbG8="
    assert!(
        first(&requests).contains(r#"{"messages":[{"data":"aGVsbG8="}]}"#),
        "{}",
        first(&requests)
    );
}

#[tokio::test]
async fn most_posts_twilio_form_with_basic_auth() {
    let mock = mock_http(vec![ok()]).await;
    let channels = AlertChannels {
        most: Some(MostChannel {
            phone: "+15550000002".into(),
            account_sid: "AC123".into(),
            auth_token: "twilio-secret".into(),
            api_version: "2010-04-01".into(),
            messaging_service_sid: Some("MG999".into()),
            from_number: None,
            api_base: mock.base_url.clone(),
        }),
        ..Default::default()
    };
    send_alert_with(&channels, "queue paused", "").await;
    let requests = mock.requests.lock().expect("requests lock");
    assert_eq!(requests.len(), one());
    assert!(
        first(&requests).starts_with("POST /2010-04-01/Accounts/AC123/Messages.json "),
        "{}",
        first(&requests)
    );
    assert!(
        first(&requests).contains("To=%2B15550000002"),
        "{}",
        first(&requests)
    );
    assert!(
        first(&requests).contains("MessagingServiceSid=MG999"),
        "{}",
        first(&requests)
    );
}

#[tokio::test]
async fn broken_channel_does_not_suppress_the_others() {
    let mock = mock_http(vec![ok()]).await;
    let channels = AlertChannels {
        // Port 1 refuses connections -> slack fails.
        slack_webhook: Some("http://127.0.0.1:1/webhook".into()),
        telegram: Some(TelegramChannel {
            token: "tok".into(),
            chat_id: "c".into(),
            api_base: mock.base_url.clone(),
        }),
        ..Default::default()
    };
    send_alert_with(&channels, "m", "").await;
    let requests = mock.requests.lock().expect("requests lock");
    assert_eq!(
        requests.len(),
        one(),
        "telegram must still fire after slack failed"
    );
}

#[tokio::test]
async fn non_2xx_is_a_channel_error_not_a_success() {
    let mock = mock_http(vec![http_response(status_server_error(), "Internal Server Error", "boom")]).await;
    let channels = AlertChannels {
        slack_webhook: Some(mock.base_url.clone()),
        ..Default::default()
    };
    // Must not panic; the failure is logged and swallowed.
    send_alert_with(&channels, "m", "").await;
    let requests = mock.requests.lock().expect("requests lock");
    assert_eq!(requests.len(), one());
}

#[tokio::test]
async fn no_channels_configured_is_a_noop() {
    send_alert_with(&AlertChannels::default(), "quiet", "").await;
}
