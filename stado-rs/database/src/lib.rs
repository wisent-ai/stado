//! A product's fleet database as a SeaORM connection.
//!
//! Every Wisent product that keeps durable state reaches it the same way:
//! Stado names the Skarbiec item holding the database's address
//! (`stado database resolve <name> --consumer <name>`), Stado's directory
//! names the Skarbiec route (`stado service directory connect skarbiec`),
//! and Skarbiec answers the pooler URL and the provider's root certificate
//! to the consumer `<name>-database-client`, whose bearer Stado keeps in
//! `~/.stado/<name>-database-client-skarbiec-token`. `connect` does those
//! steps and returns a pool verified against that certificate. Every
//! refusal names the step that failed and what Stado answered, so a product
//! never writes its own connector, row mapper or SQL client again.

mod resolve;

use std::path::PathBuf;

use sea_orm::{DatabaseConnection, SqlxPostgresConnector};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};

/// The Stado-side names of one product's database.
#[derive(Clone, Debug)]
pub struct FleetDatabase {
    /// Logical database name, as `stado database declare` recorded it.
    pub name: String,
    /// Who asks Stado's directory; the declaration lists it as consumer.
    pub directory_consumer: String,
    /// Who reads the credential item; granted exactly `pooler_url` and
    /// `ca_certificate`.
    pub credential_consumer: String,
    /// Home whose `.stado/` holds Stado and the bearer.
    pub home: PathBuf,
}

impl FleetDatabase {
    /// The names every product uses: database and directory consumer are the
    /// product, the credential reader is `<product>-database-client`. The
    /// home is `home_variable` when set (a product whose journeys give it
    /// another HOME names the account's own home there), else HOME.
    pub fn for_product(product: &str, home_variable: &str) -> Result<Self, Error> {
        let home = std::env::var_os(home_variable)
            .or_else(|| std::env::var_os("HOME"))
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| Error::new("locate Stado", format!("neither {home_variable} nor HOME is set")))?;
        Ok(Self {
            name: product.to_owned(),
            directory_consumer: product.to_owned(),
            credential_consumer: format!("{product}-database-client"),
            home,
        })
    }

    fn token_file(&self) -> PathBuf {
        self.home
            .join(".stado")
            .join(format!("{}-skarbiec-token", self.credential_consumer))
    }
}

/// One failed step of reaching the fleet database.
#[derive(Debug, thiserror::Error)]
#[error("the fleet database could not be reached at step `{step}`: {detail}")]
pub struct Error {
    pub step: &'static str,
    pub detail: String,
}

impl Error {
    pub(crate) fn new(step: &'static str, detail: impl Into<String>) -> Self {
        Self {
            step,
            detail: detail.into(),
        }
    }
}

/// Resolve the product's fleet database through Stado and Skarbiec and open
/// a SeaORM connection to it over TLS verified against the provider's root.
pub async fn connect(database: &FleetDatabase) -> Result<DatabaseConnection, Error> {
    let found = resolve::credentials(database).await?;
    let options: PgConnectOptions = found
        .pooler_url
        .parse()
        .map_err(|error| Error::new("read pooler_url", format!("{}#pooler_url is not a Postgres URL: {error}", found.item)))?;
    let options = options
        .ssl_mode(PgSslMode::VerifyFull)
        .ssl_root_cert_from_pem(found.ca_certificate.into_bytes());
    let pool = PgPoolOptions::new()
        .connect_with(options)
        .await
        .map_err(|error| {
            Error::new(
                "connect",
                format!("connecting to {} through {}#pooler_url failed: {error}", database.name, found.item),
            )
        })?;
    Ok(SqlxPostgresConnector::from_sqlx_postgres_pool(pool))
}
