//! Namecheap's answer document, read by quick-xml into the shape its API
//! documents: `ApiResponse` with a `Status` attribute, its `Errors`, and the
//! `CommandResponse` carrying a command's result. An attribute in another
//! order, an entity, a namespace or a comment reads as the value it carries,
//! and a document of another shape is refused with the parser's reason
//! instead of being searched for the words a regular expression expected.

use serde::de::DeserializeOwned;
use serde::Deserialize;

use crate::cli::CmdError;

/// The envelope of every Namecheap answer.
#[derive(Deserialize)]
pub(in crate::cli::dns) struct ApiResponse<Command> {
    #[serde(rename = "@Status")]
    status: String,
    #[serde(rename = "Errors")]
    errors: Option<Errors>,
    #[serde(rename = "CommandResponse")]
    pub(in crate::cli::dns) command: Option<Command>,
}

#[derive(Deserialize)]
struct Errors {
    #[serde(rename = "Error")]
    error: Option<Vec<ErrorText>>,
}

#[derive(Deserialize)]
struct ErrorText {
    #[serde(rename = "$text")]
    text: Option<String>,
}

/// The status Namecheap writes on an answer that carries a result.
const ANSWERED: &str = "OK";

/// What a command answered when Namecheap says it answered; Namecheap's own
/// error texts when it refused, the whole document when it gave none.
pub(in crate::cli::dns) fn read<Command: DeserializeOwned>(body: &str) -> Result<ApiResponse<Command>, CmdError> {
    let answer: ApiResponse<Command> = quick_xml::de::from_str(body).map_err(|error| {
        CmdError::click(format!("Namecheap's answer is not an API response document: {error}"))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    if answer.status == ANSWERED {
        return Ok(answer);
    }
    let errors: Vec<String> = answer
        .errors
        .and_then(|errors| errors.error)
        .into_iter()
        .flatten()
        .filter_map(|error| error.text)
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .collect();
    let said = match errors.is_empty() {
        true => body.to_string(),
        false => errors.join("; "),
    };
    Err(CmdError::refused(format!("Namecheap refused the request ({}): {said}", answer.status)))
}

/// The result one command answered with, refused naming the command when the
/// answer carries none.
pub(in crate::cli::dns) fn result<Command>(answer: ApiResponse<Command>, command: &str) -> Result<Command, CmdError> {
    answer.command.ok_or_else(|| {
        CmdError::click(format!("Namecheap answered {command} without a CommandResponse"))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })
}
