//! The registrar itself: the credential four Namecheap parameters are built
//! from, and the one POST every command in this plane goes through.
//!
//! Nothing here knows what a record is. This component answers "who are we to
//! the registrar" and "what did the registrar say", and the counted parse that
//! turns an answer into records lives beside the records.

use std::sync::LazyLock;

use crate::cli::CmdError;

use super::API;

pub(in crate::cli::dns) mod zone;

use self::zone::Zone;

/// The four fields of the registrar credential a Namecheap call needs.
///
/// Named as a set because the grant is widened over the set: asking for one
/// field at a time would leave a consumer that can read `api_user` and not
/// `api_key`, which fails on the second read of the first command anybody
/// runs.
const REGISTRAR_FIELDS: [&str; 4] = ["api_user", "api_key", "username", "client_ip"];

/// The registrar credential: four named fields, read one at a time so the
/// broker never hands over a whole item.
pub(super) struct Registrar {
    api_user: String,
    api_key: String,
    username: String,
    client_ip: String,
}

impl Registrar {
    pub(super) async fn read(item: &str) -> Result<Self, CmdError> {
        settle_readable(item).await?;
        Ok(Self {
            api_user: field(item, "api_user").await?,
            api_key: field(item, "api_key").await?,
            username: field(item, "username").await?,
            client_ip: field(item, "client_ip").await?,
        })
    }

    pub(super) fn base(&self, zone: &Zone) -> Vec<(String, String)> {
        vec![
            ("ApiUser".into(), self.api_user.clone()),
            ("ApiKey".into(), self.api_key.clone()),
            ("UserName".into(), self.username.clone()),
            ("ClientIp".into(), self.client_ip.clone()),
            ("SLD".into(), zone.sld.clone()),
            ("TLD".into(), zone.tld.clone()),
        ]
    }
}

/// Make the registrar credential readable before reading it.
///
/// A Skarbiec grant is per item and per field, so a registrar credential that
/// has sat in the vault for a long time can still answer `HTTP 403: consumer
/// not authorized to read item field`.
/// [`crate::credential_store::grant::settle_field_reads`] explains the rule.
/// The grant is made where this host's reads land: in its own vault on the
/// owner, on the owner from anywhere else.
async fn settle_readable(item: &str) -> Result<(), CmdError> {
    crate::cli::host::settle_consumer_reads(item, &REGISTRAR_FIELDS)
        .await
        .map_err(|error| {
            let failure = error.failure;
            let detail = CmdError::click(format!(
                "cannot make the registrar credential {item:?} readable: {}",
                error.message.as_deref().unwrap_or("no detail")
            ));
            CmdError { failure, ..detail }
        })
}

/// `--credential` names the item, so it is read as named. Looking it up as a
/// role found no item playing a role called by that name and reported a field
/// that is in the vault as missing.
async fn field(item: &str, name: &str) -> Result<String, CmdError> {
    crate::credential_store::read_declared_string(item, name)
        .await
        .map_err(CmdError::from)?
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CmdError::click(format!(
                "credential field {name:?} of {item:?} is required; \
                 the registrar credential carries api_user, api_key, username and client_ip"
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound)
        })
}

pub(super) static HOST_ELEMENT: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?s)<host\b[^>]*/?>").expect("static regex compiles"));
pub(super) static ATTRIBUTE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#"([A-Za-z]+)="([^"]*)""#).expect("static regex compiles"));
static ERROR_ELEMENT: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?s)<Error[^>]*>(.*?)</Error>").expect("static regex compiles")
});

/// XML attribute text, with the five entities an attribute value can carry.
pub(super) fn unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// POST one command and return the response body, refusing a non-OK status
/// with the registrar's own error text.
pub(super) async fn call(parameters: Vec<(String, String)>) -> Result<String, CmdError> {
    let client = reqwest::Client::builder().build().map_err(|error| {
        CmdError::click(format!(
            "the Namecheap HTTP client cannot be built: {error}"
        ))
        .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let response = crate::wait::request(client.post(API).form(&parameters))
        .await
        .map_err(|error| {
            CmdError::click(format!("Namecheap API is unreachable: {error}"))
                .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
    let status = response.status();
    let body = response.text().await.map_err(|error| {
        CmdError::click(format!("Namecheap's answer could not be read: {error}"))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    if !status.is_success() {
        return Err(
            CmdError::click(format!("Namecheap answered HTTP {status}")).stating(
                crate::primitives::failure::FailureCode::from_upstream_status(status.as_u16()),
            ),
        );
    }
    if !body.contains(r#"Status="OK""#) {
        let errors: Vec<String> = ERROR_ELEMENT
            .captures_iter(&body)
            .map(|capture| unescape(capture[1].trim()))
            .filter(|text| !text.is_empty())
            .collect();
        return Err(CmdError::refused(format!(
            "Namecheap refused the request: {}",
            if errors.is_empty() {
                body.to_string()
            } else {
                errors.join("; ")
            }
        )));
    }
    Ok(body)
}
