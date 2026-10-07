//! The preflight gates: one per registration path, each refusing with the
//! registry field that switched that path off. A catalog that does not parse
//! is the registry's configuration; a path the catalog switched off is
//! refused.

use serde_json::Value;

use crate::cli::CmdError;

use super::sections::parse_enrollment;

/// Gate for machine-initiated registration.
pub fn require_join_allowed(document: &Value) -> Result<(), CmdError> {
    let catalog = parse_enrollment(document).map_err(CmdError::declaration)?;
    if !catalog.allow_join {
        return Err(CmdError::refused(
            "machine-initiated enrollment is disabled by registry.enrollment.allow_join",
        ));
    }
    Ok(())
}

/// Gate for control-plane-initiated registration.
pub fn require_enroll_allowed(document: &Value) -> Result<(), CmdError> {
    let catalog = parse_enrollment(document).map_err(CmdError::declaration)?;
    if !catalog.allow_enroll {
        return Err(CmdError::refused(
            "control-plane enrollment is disabled by registry.enrollment.allow_enroll",
        ));
    }
    Ok(())
}

/// Gate for invite-based registration: the operator mints a token, the
/// machine's owner runs one line.
pub fn require_invite_allowed(document: &Value) -> Result<(), CmdError> {
    let catalog = parse_enrollment(document).map_err(CmdError::declaration)?;
    if !catalog.allow_invite {
        return Err(CmdError::refused(
            "invite-based enrollment is disabled by registry.enrollment.allow_invite",
        ));
    }
    Ok(())
}

/// Gate for adoption: the operator already has an SSH session, so Stado
/// installs the fleet's public key itself.
pub fn require_adopt_allowed(document: &Value) -> Result<(), CmdError> {
    let catalog = parse_enrollment(document).map_err(CmdError::declaration)?;
    if !catalog.allow_adopt {
        return Err(CmdError::refused(
            "adoption is disabled by registry.enrollment.allow_adopt",
        ));
    }
    Ok(())
}
