//! The preflight gates: one per registration path, each refusing with the
//! registry field that switched that path off and the methods the fleet does
//! allow. A catalog that does not parse is the registry's configuration; a
//! path the catalog switched off is refused.

use serde_json::Value;

use crate::cli::fleet::enroll::failures::{
    ADOPT_DISABLED, CATALOG_INVALID, ENROLL_DISABLED, INVITE_DISABLED, JOIN_DISABLED,
};
use crate::cli::CmdError;

use super::sections::{parse_enrollment, EnrollmentCatalog};

/// The catalog, or the declared cause it cannot be read as.
fn read(document: &Value) -> Result<EnrollmentCatalog, CmdError> {
    parse_enrollment(document)
        .map_err(|answer| CmdError::declared(&CATALOG_INVALID, &[("answer", &answer)]))
}

/// The methods the catalog allows, as the refusals name them.
fn allowed(catalog: &EnrollmentCatalog) -> String {
    let methods: Vec<&str> = [
        ("invite", catalog.allow_invite),
        ("adopt", catalog.allow_adopt),
        ("join", catalog.allow_join),
        ("enroll", catalog.allow_enroll),
    ]
    .into_iter()
    .filter_map(|(method, allowed)| allowed.then_some(method))
    .collect();
    if methods.is_empty() {
        "none".to_string()
    } else {
        methods.join(", ")
    }
}

/// Gate for machine-initiated registration of `name`.
pub fn require_join_allowed(document: &Value, name: &str) -> Result<(), CmdError> {
    let catalog = read(document)?;
    if !catalog.allow_join {
        return Err(CmdError::declared(
            &JOIN_DISABLED,
            &[("name", name), ("allowed", &allowed(&catalog))],
        ));
    }
    Ok(())
}

/// Gate for control-plane-initiated registration of `name`.
pub fn require_enroll_allowed(document: &Value, name: &str) -> Result<(), CmdError> {
    let catalog = read(document)?;
    if !catalog.allow_enroll {
        return Err(CmdError::declared(
            &ENROLL_DISABLED,
            &[("name", name), ("allowed", &allowed(&catalog))],
        ));
    }
    Ok(())
}

/// Gate for approving a machine an invite brought in, `name` reserved by the
/// invite and `hostname` its request.
pub fn require_invited_approval(
    document: &Value,
    name: &str,
    hostname: &str,
) -> Result<(), CmdError> {
    let catalog = read(document)?;
    if !catalog.allow_invite {
        return Err(CmdError::declared(
            &INVITE_DISABLED,
            &[
                ("name", name),
                ("hostname", hostname),
                ("allowed", &allowed(&catalog)),
            ],
        ));
    }
    Ok(())
}

/// Gate for minting an invite: the operator mints a token, the machine's
/// owner runs one line.
pub fn require_invite_allowed(document: &Value) -> Result<(), CmdError> {
    let catalog = parse_enrollment(document).map_err(CmdError::declaration)?;
    if !catalog.allow_invite {
        return Err(CmdError::refused(
            "invite-based enrollment is disabled by registry.enrollment.allow_invite",
        ));
    }
    Ok(())
}

/// Gate for adoption: the operator already has an SSH session to `ssh`, so
/// Stado installs the fleet's public key itself.
pub fn require_adopt_allowed(document: &Value, ssh: &str) -> Result<(), CmdError> {
    let catalog = read(document)?;
    if !catalog.allow_adopt {
        return Err(CmdError::declared(
            &ADOPT_DISABLED,
            &[("ssh", ssh), ("allowed", &allowed(&catalog))],
        ));
    }
    Ok(())
}
