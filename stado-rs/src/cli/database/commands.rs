//! The `stado database` command surface: one clap subcommand per verb and
//! read, with the plane's contract carried in their help text.

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub(crate) enum DatabaseCommands {
    /// List declared databases and whether each is placed.
    List {
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Resolve one database for an authorized consumer.
    ///
    /// Returns the placement endpoint when the service directory places the
    /// database, and always the Skarbiec item to acquire the credential
    /// from. The consumer must be declared on the database; the credential
    /// value is never printed.
    Resolve {
        /// Logical database name from database_api.databases.
        name: String,
        /// Stable workload identity requesting access.
        #[arg(long)]
        consumer: String,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Declare a database in the Stado configuration.
    ///
    /// Writes `database_api.databases.<name>` through the same validated,
    /// atomic write every other configuration change uses. The credential
    /// item `<name>-database` is implied; provision its fields with
    /// `stado secrets put <name>-database`.
    Declare {
        /// Logical database name (lowercase letters, digits, dashes).
        name: String,
        /// Database engine.
        #[arg(long)]
        engine: String,
        /// Access scopes to grant the declaration (read, write).
        #[arg(long = "scope", value_delimiter = ',')]
        scopes: Vec<String>,
        /// Consumer allowed to resolve this database.
        #[arg(long = "consumer", value_delimiter = ',', required = true)]
        consumers: Vec<String>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Remove a database declaration from the Stado configuration.
    Remove {
        name: String,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Grant one or more consumers access to a declared database.
    Grant {
        name: String,
        #[arg(long = "consumer", value_delimiter = ',', required = true)]
        consumers: Vec<String>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Revoke one or more consumers' access to a declared database.
    Revoke {
        name: String,
        #[arg(long = "consumer", value_delimiter = ',', required = true)]
        consumers: Vec<String>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
}
