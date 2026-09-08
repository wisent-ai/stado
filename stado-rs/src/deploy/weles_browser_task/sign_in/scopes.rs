//! The registered identities: which name a capability for one vault
//! coordinate must be issued to, read off the catalog the host's own workload
//! registrations were minted from.

use crate::deploy::{service_file_fetch, DeployError, Runner};
use crate::targets::ComputeTarget;

/// The acquisition scopes catalog as the host registered it.
///
/// `stado host sync-acquisition-scopes` delivers the checked-in catalog to
/// `$HOME/.stado/files/` and runs `skarbiec token-register-acquisitions` from
/// THAT copy, so this staged file — not the release tree's — is the document
/// the vault's workload registrations were minted from.
pub const REGISTERED_SCOPES_FILE: &str = "$HOME/.stado/files/skarbiec-acquisition-scopes.conf";

/// One catalog row: a name a workload public key is registered under, and the
/// vault coordinate that grant covers.
#[derive(Debug, PartialEq, Eq)]
pub struct AcquisitionScope {
    pub consumer: String,
    pub item: String,
    pub field: String,
}

/// Every registered name in one catalog, in the order the file lists them.
///
/// `consumer|item|field` per line, `#` comments and blank lines ignored — the
/// shape `read_acquisition_catalog` parses on the host. A row this cannot read
/// is skipped rather than guessed at: the vault, not this parse, decides what
/// is registered, and a wrong guess here would name an identity that is not.
pub fn parse_scopes(body: &str) -> Vec<AcquisitionScope> {
    let mut scopes = Vec::new();
    for line in body.lines() {
        let row = line.trim();
        if row.is_empty() || row.starts_with('#') {
            continue;
        }
        let mut columns = row.split('|');
        let (Some(consumer), Some(item), Some(field), None) = (
            columns.next(),
            columns.next(),
            columns.next(),
            columns.next(),
        ) else {
            continue;
        };
        if consumer.is_empty() || item.is_empty() || field.is_empty() {
            continue;
        }
        scopes.push(AcquisitionScope {
            consumer: consumer.to_string(),
            item: item.to_string(),
            field: field.to_string(),
        });
    }
    scopes
}

/// The name a capability for one vault coordinate must be issued to.
///
/// Skarbiec authorises a redemption by the live vault token registering that
/// workload's Ed25519 key, and it looks that token up by the capability's
/// agent — by name, with no capability check of its own. On this fleet the
/// worker holds one key whose public half is registered under the catalog's
/// per-field consumer names, so the registered name for the coordinate being
/// filled is the only agent whose signature can verify.
///
/// Naming anything else is denied however correct the purpose, resource and
/// route are: run 18e7cc47 was refused for `weles-worker`, a constant copied
/// from the Apple sign-in, and run 47d89182 for `weles-credential-worker-local`,
/// the worker's own `SKARBIEC_WORKLOAD_ID` — that string labels the workload,
/// it is not a registration.
pub fn scope_consumer<'a>(
    scopes: &'a [AcquisitionScope],
    item: &str,
    field: &str,
) -> Option<&'a str> {
    scopes
        .iter()
        .find(|scope| scope.item == item && scope.field == field)
        .map(|scope| scope.consumer.as_str())
}

/// Read the catalog the host's workload registrations were minted from.
pub async fn host_scopes(
    target: &ComputeTarget,
    scopes_file: &str,
    runner: &Runner,
) -> Result<Vec<AcquisitionScope>, DeployError> {
    let fetched = service_file_fetch::fetch_file(target, scopes_file, runner).await?;
    if !fetched.ok() {
        return Err(DeployError(format!(
            "{}: could not read {scopes_file} to learn which identities its vault registers: {}",
            target.name, fetched.report.file_state
        )));
    }
    Ok(parse_scopes(&String::from_utf8_lossy(&fetched.content)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_identity_for_a_coordinate_is_the_one_the_catalog_registers_for_it() {
        // charless-mac-mini's own catalog, four rows of it: one consumer per
        // (item, field), which is why the agent cannot be a single per-host
        // name.
        let body = "# consumer|item|field\n\
                    \n\
                    weles-gmail-client-username|weles-gmail-login|username\n\
                    weles-google-sso-client-username|weles-google-sso-login|username\n\
                    weles-google-sso-client-password|weles-google-sso-login|password\n";
        let scopes = parse_scopes(body);
        assert_eq!(scopes.len(), 3);
        assert_eq!(
            scope_consumer(&scopes, "weles-google-sso-login", "username"),
            Some("weles-google-sso-client-username")
        );
        assert_eq!(
            scope_consumer(&scopes, "weles-google-sso-login", "password"),
            Some("weles-google-sso-client-password")
        );
    }

    #[test]
    fn a_coordinate_the_catalog_never_registers_has_no_identity_to_issue_to() {
        // The two denials this replaced: names that sound like the worker but
        // register nothing. An unregistered coordinate must answer None so the
        // caller refuses before spending a capability the broker would deny.
        let scopes =
            parse_scopes("weles-google-sso-client-username|weles-google-sso-login|username\n");
        assert_eq!(
            scope_consumer(&scopes, "weles-google-sso-login", "totp_secret"),
            None
        );
        assert_eq!(scope_consumer(&scopes, "weles-worker", "username"), None);
    }

    #[test]
    fn a_row_this_cannot_read_is_skipped_rather_than_guessed_at() {
        let scopes = parse_scopes(
            "# comment\n\
             \n\
             two|columns\n\
             four|too|many|columns\n\
             |weles-google-sso-login|username\n\
             weles-google-sso-client-username|weles-google-sso-login|username\n",
        );
        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].consumer, "weles-google-sso-client-username");
    }
}
