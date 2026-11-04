//! The license a model repository declares on the Hugging Face hub, and the
//! licenses of the base models it names, read for one pinned revision.
//!
//! `stado inference plan` records what this answers in the deployment's
//! `model.license`, so whether the model may be sold is a fact the registry
//! holds beside the repository and revision, read once at the revision the
//! engine loads. A repository that declares no license is recorded as
//! declaring none, never as a guess; a base model that declares none is named
//! the same way. The read uses the token of the Skarbiec `huggingface` role
//! when one is stored, as the artifact adapters do, so a gated repository is
//! read with the operator's access.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One repository's declared license at one revision.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Declared {
    pub repository: String,
    /// The `license` of the model card, or `None` when the card declares
    /// none.
    pub license: Option<String>,
}

/// A deployment's model license: the repository's own and each base model's.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct License {
    pub model: Declared,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub base: Vec<Declared>,
}

impl License {
    /// Every repository of the chain that declares no license, the model
    /// first; empty when each declares one.
    pub fn undeclared(&self) -> Vec<&str> {
        std::iter::once(&self.model)
            .chain(&self.base)
            .filter(|declared| declared.license.is_none())
            .map(|declared| declared.repository.as_str())
            .collect()
    }

    /// `repo: license` for each repository of the chain, `none` where the
    /// card declares none.
    pub fn summary(&self) -> String {
        std::iter::once(&self.model)
            .chain(&self.base)
            .map(|declared| {
                format!(
                    "{}: {}",
                    declared.repository,
                    declared.license.as_deref().unwrap_or("none declared")
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// What the hub's model card declares for `repository` at `revision`: its
/// license and the base models it names.
async fn card(
    client: &reqwest::Client,
    token: &str,
    repository: &str,
    revision: &str,
) -> Result<(Option<String>, Vec<String>), String> {
    let url = format!("https://huggingface.co/api/models/{repository}/revision/{revision}");
    let mut request = client.get(&url);
    if !token.is_empty() {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let response = crate::wait::request(request)
        .await
        .map_err(|error| format!("the model hub could not be asked for {repository}: {error}"))?;
    let status = response.status();
    if !status.is_success() {
        // The hub answers 401 for a repository that does not exist or that
        // the token may not read; either way nothing is known about it.
        return Err(format!(
            "the model hub answered {status} for {repository}@{revision}: the repository does not \
             exist, or it is gated and the Skarbiec role huggingface holds no token that may read it"
        ));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|error| format!("the model hub's answer for {repository} is not JSON: {error}"))?;
    let card = body.get("cardData");
    let license = card
        .and_then(|card| card.get("license"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|license| !license.trim().is_empty());
    // A card names its base model as one string or a list of them.
    let base = match card.and_then(|card| card.get("base_model")) {
        Some(Value::String(one)) => vec![one.clone()],
        Some(Value::Array(many)) => many
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    };
    Ok((license, base))
}

/// The license `repository` declares at `revision`, with its base models'
/// licenses at their current main revision (a card names a base by
/// repository, not by revision).
pub async fn read(repository: &str, revision: &str) -> Result<License, String> {
    let client = reqwest::Client::builder()
        .user_agent("stado-inference/1")
        .build()
        .map_err(|error| format!("no HTTP client for the model hub: {error}"))?;
    let token = crate::skarbiec::read_string("huggingface", "token")
        .await
        .map_err(|error| format!("the Skarbiec role huggingface could not be read: {error}"))?
        .unwrap_or_default();
    let (license, bases) = card(&client, &token, repository, revision).await?;
    let mut base = Vec::with_capacity(bases.len());
    for name in bases {
        let (license, _) = card(&client, &token, &name, "main").await?;
        base.push(Declared {
            repository: name,
            license,
        });
    }
    Ok(License {
        model: Declared {
            repository: repository.to_string(),
            license,
        },
        base,
    })
}
