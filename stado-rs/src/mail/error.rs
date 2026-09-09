//! The single error type every Gmail entry point returns.

#[derive(Debug, thiserror::Error)]
pub enum MailError {
    #[error("Gmail authentication unavailable: {0}")]
    Auth(String),
    #[error("Gmail API HTTP {status}: {detail}")]
    Api { status: u16, detail: String },
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}
