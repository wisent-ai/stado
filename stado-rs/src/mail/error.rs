//! Why a read of Skrzynka's messages produced nothing.

#[derive(Debug, thiserror::Error)]
pub enum MailError {
    #[error("Skrzynka could not be run: {0}")]
    Unreachable(String),
    #[error("Skrzynka refused ({status}): {detail}")]
    Refused { status: String, detail: String },
    #[error("Skrzynka answered something that is not a message list: {0}")]
    Unreadable(String),
}
