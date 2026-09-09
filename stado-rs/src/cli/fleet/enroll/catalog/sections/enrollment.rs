//! The `enrollment` section: which registration paths the fleet allows, and
//! which key custody it declares.

use serde_json::Value;

/// The parsed `enrollment` section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrollmentCatalog {
    pub declared: bool,
    pub allow_join: bool,
    pub allow_enroll: bool,
    pub allow_invite: bool,
    pub allow_adopt: bool,
    pub require_verified_hostname: bool,
    pub key_custody: String,
}

/// Key custody values the fleet supports.
const CUSTODY_SKARBIEC: &str = "skarbiec";
const CUSTODY_OPENSSH: &str = "openssh";

fn bool_field(section: &Value, key: &str, location: &str) -> Result<bool, String> {
    match section.get(key) {
        None => Ok(false),
        Some(value) => value
            .as_bool()
            .ok_or_else(|| format!("{location}.{key}: must be a boolean")),
    }
}

/// Parse the optional `enrollment` section; undeclared means every path is
/// allowed and hostname verification is not required (today's behavior,
/// reported as unrestricted by `catalog`).
pub fn parse_enrollment(document: &Value) -> Result<EnrollmentCatalog, String> {
    let Some(section) = document.get("enrollment") else {
        return Ok(EnrollmentCatalog {
            declared: false,
            allow_join: true,
            allow_enroll: true,
            allow_invite: true,
            allow_adopt: true,
            require_verified_hostname: false,
            key_custody: CUSTODY_SKARBIEC.to_string(),
        });
    };
    if !section.is_object() {
        return Err("registry.enrollment: must be an object".to_string());
    }
    let location = "registry.enrollment";
    // Every allowance defaults to permitted: an `enrollment` section written
    // before a method existed must not silently forbid that method.
    let allowance = |key: &str| -> Result<bool, String> {
        match section.get(key) {
            None => Ok(true),
            Some(_) => bool_field(section, key, location),
        }
    };
    let key_custody = match section.get("key_custody") {
        None => CUSTODY_SKARBIEC.to_string(),
        Some(value) => {
            let custody = value
                .as_str()
                .ok_or_else(|| format!("{location}.key_custody: must be a string"))?;
            if custody != CUSTODY_SKARBIEC && custody != CUSTODY_OPENSSH {
                return Err(format!(
                    "{location}.key_custody: must be '{CUSTODY_SKARBIEC}' or '{CUSTODY_OPENSSH}'"
                ));
            }
            custody.to_string()
        }
    };
    Ok(EnrollmentCatalog {
        declared: true,
        allow_join: allowance("allow_join")?,
        allow_enroll: allowance("allow_enroll")?,
        allow_invite: allowance("allow_invite")?,
        allow_adopt: allowance("allow_adopt")?,
        require_verified_hostname: bool_field(section, "require_verified_hostname", location)?,
        key_custody,
    })
}
