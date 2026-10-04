//! The admission bearer token, as Stado's credential store on THIS machine
//! answers for it, and the refusal wording each answer earns.

use super::super::{ADMISSION_TOKEN_FIELD, ADMISSION_TOKEN_ROLE};

/// What Stado's credential store had to say about the admission bearer token.
#[derive(Debug, Clone)]
pub(super) enum Token {
    /// The store holds one, and every request carries it.
    Present(String),
    /// The store answered and holds none — the documented state for a
    /// listener bound to loopback, which serves unauthenticated.
    Absent,
    /// The store did not answer. Not fatal by itself, because a loopback
    /// listener may want no token at all; the API's own 401 is what decides,
    /// and this string is what that refusal then reports.
    Unreadable(String),
}

impl Token {
    pub(super) fn describe(&self) -> String {
        match self {
            Self::Present(_) => format!(
                "the {ADMISSION_TOKEN_FIELD} of the item playing role {ADMISSION_TOKEN_ROLE} was rejected"
            ),
            Self::Absent => format!(
                "no item in Stado's credential store plays role {ADMISSION_TOKEN_ROLE} with a {ADMISSION_TOKEN_FIELD}; \
                 tag the item holding the admission token stado:role:{ADMISSION_TOKEN_ROLE}"
            ),
            Self::Unreadable(error) => format!(
                "Stado's credential store could not be read for role {ADMISSION_TOKEN_ROLE} field {ADMISSION_TOKEN_FIELD}: {error}"
            ),
        }
    }

    pub(super) fn state(&self) -> &'static str {
        match self {
            Self::Present(_) => "present",
            Self::Absent => "absent",
            Self::Unreadable(_) => "unreadable",
        }
    }
}

pub(super) async fn read_token() -> Token {
    match crate::credential_store::read_string(ADMISSION_TOKEN_ROLE, ADMISSION_TOKEN_FIELD).await {
        Ok(Some(token)) if !token.trim().is_empty() => Token::Present(token.trim().to_string()),
        Ok(_) => Token::Absent,
        Err(error) => Token::Unreadable(error.to_string()),
    }
}
