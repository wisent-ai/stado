//! The preflight gates: one per registration path, each refusing with the
//! registry field that switched that path off.

use serde_json::Value;

use super::sections::parse_enrollment;

/// Gate for machine-initiated registration.
pub fn require_join_allowed(document: &Value) -> Result<(), String> {
    let catalog = parse_enrollment(document)?;
    if !catalog.allow_join {
        return Err(
            "machine-initiated enrollment is disabled by registry.enrollment.allow_join"
                .to_string(),
        );
    }
    Ok(())
}

/// Gate for control-plane-initiated registration.
pub fn require_enroll_allowed(document: &Value) -> Result<(), String> {
    let catalog = parse_enrollment(document)?;
    if !catalog.allow_enroll {
        return Err(
            "control-plane enrollment is disabled by registry.enrollment.allow_enroll".to_string(),
        );
    }
    Ok(())
}

/// Gate for invite-based registration: the operator mints a token, the
/// machine's owner runs one line.
pub fn require_invite_allowed(document: &Value) -> Result<(), String> {
    let catalog = parse_enrollment(document)?;
    if !catalog.allow_invite {
        return Err(
            "invite-based enrollment is disabled by registry.enrollment.allow_invite".to_string(),
        );
    }
    Ok(())
}

/// Gate for adoption: the operator already has an SSH session, so Stado
/// installs the fleet's public key itself.
pub fn require_adopt_allowed(document: &Value) -> Result<(), String> {
    let catalog = parse_enrollment(document)?;
    if !catalog.allow_adopt {
        return Err("adoption is disabled by registry.enrollment.allow_adopt".to_string());
    }
    Ok(())
}
