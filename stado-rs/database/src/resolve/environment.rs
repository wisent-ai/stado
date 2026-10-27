//! A database named in the product's own environment, for a machine without
//! Stado or Skarbiec.

use std::path::PathBuf;

use crate::{Error, FleetDatabase};

use super::{is_file_url, is_postgres_url, Credentials};

/// The database a product names in its own environment, for a machine
/// without Stado or Skarbiec: `<PRODUCT>_DATABASE_URL` (dashes become
/// underscores, letters upper case) and, for a server, `<PRODUCT>_DATABASE_CA_FILE`,
/// the PEM bundle it is verified against. Nothing when the URL is unset; a
/// refusal naming the variable when a server's certificate is not set.
pub(super) fn from_environment(database: &FleetDatabase) -> Option<Result<Credentials, Error>> {
    let prefix = database.name.to_uppercase().replace('-', "_");
    let url_variable = format!("{prefix}_DATABASE_URL");
    let url = match std::env::var(&url_variable) {
        Ok(value) if value.trim().is_empty() => return None,
        Ok(value) => value.trim().to_string(),
        Err(std::env::VarError::NotPresent) => return None,
        Err(std::env::VarError::NotUnicode(raw)) => {
            return Some(Err(Error::new(
                "read environment",
                format!("{url_variable} is set but is not Unicode text: {raw:?}"),
            )))
        }
    };
    if is_file_url(&url) {
        return Some(Ok(Credentials {
            item: url_variable,
            pooler_url: url,
            session_url: None,
            ca_certificate: String::new(),
        }));
    }
    let ca_variable = format!("{prefix}_DATABASE_CA_FILE");
    let Some(ca_file) = std::env::var_os(&ca_variable).filter(|value| !value.is_empty()) else {
        return Some(Err(Error::new(
            "read environment",
            format!("{url_variable} is set but {ca_variable} is not; the server is verified against that certificate"),
        )));
    };
    Some(
        std::fs::read_to_string(&ca_file)
            .map_err(|error| {
                Error::new(
                    "read environment",
                    format!(
                        "{ca_variable}={}: {error}",
                        PathBuf::from(&ca_file).display()
                    ),
                )
            })
            .map(|ca_certificate| Credentials {
                item: url_variable,
                // The one URL the product names is the one it is opened on.
                session_url: is_postgres_url(&url).then(|| url.clone()),
                pooler_url: url,
                ca_certificate,
            }),
    )
}
