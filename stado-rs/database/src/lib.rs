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
pub use resolve::Credentials;
pub mod sync;

use std::path::PathBuf;

use sea_orm::{DatabaseConnection, SqlxMySqlConnector, SqlxPostgresConnector, SqlxSqliteConnector};
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions, MySqlSslMode};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

/// The Stado-side names of one product's database.
#[derive(Clone, Debug)]
pub struct FleetDatabase {
    /// Logical database name, as `stado database declare` recorded it.
    pub name: String,
    /// Who asks Stado's directory; the declaration lists it as consumer.
    pub directory_consumer: String,
    /// Who reads the credential item: `pooler_url`, `ca_certificate` and, for
    /// Postgres, `session_url`.
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
            .ok_or_else(|| {
                Error::new(
                    "locate Stado",
                    format!("neither {home_variable} nor HOME is set"),
                )
            })?;
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

/// Resolve the product's fleet database through Stado and Skarbiec and
/// answer its connection URL and certificate authority, for an engine
/// SeaORM does not speak (MongoDB, Redis, SQL Server, ...): the product opens
/// it with that engine's own driver, verified against `ca_certificate`.
pub async fn credentials(database: &FleetDatabase) -> Result<Credentials, Error> {
    resolve::credentials(database).await
}

/// Resolve the product's fleet database through Stado and Skarbiec and open
/// a SeaORM connection to it. The URL's scheme says which database it is:
/// `postgres://` or `postgresql://` is Postgres, opened on the item's
/// `session_url`, and `mysql://` is MySQL, both over TLS verified against
/// the provider's root; `sqlite://<file>` is a fleet SQLite file, opened on
/// the host that holds it. Any other engine is refused with its scheme
/// named; it is opened from [`credentials`].
pub async fn connect(database: &FleetDatabase) -> Result<DatabaseConnection, Error> {
    let found = resolve::credentials(database).await?;
    match found.pooler_url.split_once("://").map(|(scheme, _)| scheme) {
        Some("mysql") => return connect_mysql(database, found).await,
        Some("sqlite") => return connect_sqlite(database, found).await,
        Some("postgres" | "postgresql") => {}
        scheme => {
            return Err(Error::new(
                "read pooler_url",
                format!(
                    "{}#pooler_url is a {} URL; connect opens postgres, mysql and sqlite through SeaORM, so open this engine with its own driver from stado_database::credentials",
                    found.item,
                    scheme.unwrap_or("schemeless")
                ),
            ))
        }
    }
    let Some(session_url) = found.session_url.as_deref() else {
        return Err(Error::new(
            "read session_url",
            format!(
                "{} names a Postgres database without a session_url",
                found.item
            ),
        ));
    };
    let options: PgConnectOptions = session_url.parse().map_err(|error| {
        Error::new(
            "read session_url",
            format!("{}#session_url is not a Postgres URL: {error}", found.item),
        )
    })?;
    // SeaORM sends every statement persistent, so sqlx names each one
    // (`sqlx_s_1`, ...) on the server connection it ran on. A
    // transaction-mode pooler (`pooler_url` of a Supabase database) hands the
    // next transaction another server connection, where another client's
    // statement of that name already exists. The session route keeps one
    // server connection per client connection, so a name stays its own.
    let options = options
        .ssl_mode(PgSslMode::VerifyFull)
        .ssl_root_cert_from_pem(found.ca_certificate.into_bytes());
    let pool = stado_wait::until(
        stado_wait::Kind::Database,
        format!("connect to {}", database.name),
        format!("{}#session_url", found.item),
        PgPoolOptions::new().connect_with(options),
    )
    .await
    .map_err(|error| {
        Error::new(
            "connect",
            format!(
                "connecting to {} through {}#session_url failed: {error}",
                database.name, found.item
            ),
        )
    })?;
    Ok(SqlxPostgresConnector::from_sqlx_postgres_pool(pool))
}

async fn connect_mysql(
    database: &FleetDatabase,
    found: resolve::Credentials,
) -> Result<DatabaseConnection, Error> {
    let options: MySqlConnectOptions = found.pooler_url.parse().map_err(|error| {
        Error::new(
            "read pooler_url",
            format!("{}#pooler_url is not a MySQL URL: {error}", found.item),
        )
    })?;
    let options = options
        .ssl_mode(MySqlSslMode::VerifyIdentity)
        .ssl_ca_from_pem(found.ca_certificate.into_bytes());
    let pool = stado_wait::until(
        stado_wait::Kind::Database,
        format!("connect to {}", database.name),
        format!("{}#pooler_url", found.item),
        MySqlPoolOptions::new().connect_with(options),
    )
    .await
    .map_err(|error| {
        Error::new(
            "connect",
            format!(
                "connecting to {} through {}#pooler_url failed: {error}",
                database.name, found.item
            ),
        )
    })?;
    Ok(SqlxMySqlConnector::from_sqlx_mysql_pool(pool))
}

/// A fleet SQLite database is a file on one host. It is opened as it is,
/// never created here: a missing file means this is not the host that holds
/// it, and the refusal names the file.
async fn connect_sqlite(
    database: &FleetDatabase,
    found: resolve::Credentials,
) -> Result<DatabaseConnection, Error> {
    let options: SqliteConnectOptions = found.pooler_url.parse().map_err(|error| {
        Error::new(
            "read pooler_url",
            format!("{}#pooler_url is not a SQLite URL: {error}", found.item),
        )
    })?;
    let file = options.get_filename().display().to_string();
    let pool = stado_wait::until(
        stado_wait::Kind::Database,
        format!("open {}", database.name),
        file.as_str(),
        SqlitePoolOptions::new().connect_with(options.create_if_missing(false)),
    )
    .await
        .map_err(|error| {
            Error::new(
                "connect",
                format!(
                    "opening {} at {file} through {}#pooler_url failed (a fleet SQLite file opens only on the host that holds it): {error}",
                    database.name, found.item
                ),
            )
        })?;
    Ok(SqlxSqliteConnector::from_sqlx_sqlite_pool(pool))
}
