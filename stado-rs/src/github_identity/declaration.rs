//! The GitHub identity declaration held to its rules: one exact route
//! Skarbiec answers from its route table, and the reality check that proves
//! it.

use super::{GithubIdentity, DECLARATION, DECLARATION_PATH, SCHEMA, SELF_DECLARING_PREFIXES};

fn exact(value: &str, field: &str) -> Result<(), String> {
    if value.is_empty()
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err(format!(
            "{DECLARATION_PATH} {field} must be one exact non-empty token"
        ));
    }
    Ok(())
}

/// Parse the compiled-in declaration and refuse one that breaks a rule,
/// naming the declaration file and the field.
pub(super) fn parse_declaration() -> Result<GithubIdentity, String> {
    let identity: GithubIdentity = serde_json::from_str(DECLARATION)
        .map_err(|error| format!("{DECLARATION_PATH} is invalid: {error}"))?;
    if identity.schema != SCHEMA {
        return Err(format!(
            "{DECLARATION_PATH} schema is {:?}, expected {SCHEMA:?}",
            identity.schema
        ));
    }
    exact(&identity.credential_route, "credential_route")?;
    exact(&identity.required_permission, "required_permission")?;
    exact(&identity.reality_check, "reality_check")?;
    // Skarbiec resolves `provider:`, `agent:` and `login:` from what a vault
    // item declares about itself, and `<item>#<field>` is an exact coordinate
    // named rather than declared. A fleet GitHub identity is neither: it is a
    // resource no item can declare, so it belongs in Skarbiec's hand-declared
    // route table and has to be shaped like one. Accepting a coordinate here
    // would put the item id back in Stado's tree under a different name.
    if identity.credential_route.contains('#')
        || SELF_DECLARING_PREFIXES
            .iter()
            .any(|prefix| identity.credential_route.starts_with(prefix))
        || !identity.credential_route.contains(':')
    {
        return Err(format!(
            "{DECLARATION_PATH} credential_route {:?} is not a declarable route: name a \
             <product>:<role> resource Skarbiec answers from its route table, such as \
             \"github:org-runner-admin\", not an item coordinate and not a prefix an item \
             declares for itself",
            identity.credential_route
        ));
    }
    if !identity.reality_check.starts_with('/') {
        return Err(format!(
            "{DECLARATION_PATH} reality_check must be an api.github.com path beginning with \"/\""
        ));
    }
    if identity.reality_check.contains("{repository}") {
        let repository = identity
            .reality_check_repository
            .as_deref()
            .unwrap_or_default();
        exact(repository, "reality_check_repository")?;
        if repository.contains('/') {
            return Err(format!(
                "{DECLARATION_PATH} reality_check_repository {repository:?} must be one \
                 repository name inside the organization"
            ));
        }
    }
    Ok(identity)
}
