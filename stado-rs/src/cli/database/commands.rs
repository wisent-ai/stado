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
    /// `stado credentials put <name>-database`.
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
    /// Create a database the fleet does not have yet, then declare it.
    ///
    /// Any engine Stado declares (`--engine`: postgres, sqlite or mysql) on any
    /// provider (`--provider`). The default provider is `fleet`: Stado runs
    /// the database itself on one fleet host -- the vault owner unless
    /// `--host` names another -- with no vendor and no bill, through `stado
    /// database place` on that host. `supabase` creates a hosted Postgres
    /// project in the organization and region of `--anchor`'s project, or,
    /// without it, the one organization and region every project the token
    /// sees shares, and refuses unless `--accept-monthly-usd` covers what one
    /// more project adds to the compute bill. `external` brings a Postgres or
    /// MySQL server the user already runs anywhere (RDS, Cloud SQL, Neon,
    /// PlanetScale, Azure, their own server): the connection URL is read from
    /// standard input and
    /// `--ca-certificate` names the server's certificate authority. Every way,
    /// the credential item `<name>-database` is written into the owner vault
    /// and the database is declared for its consumers. A fleet or supabase
    /// database of that name is reused, never duplicated; an external one is
    /// re-pointed at the URL given.
    Create {
        /// Logical database name (lowercase letters, digits, dashes).
        name: String,
        /// Consumer allowed to resolve this database.
        #[arg(long = "consumer", value_delimiter = ',', required = true)]
        consumers: Vec<String>,
        /// Engine the database speaks: postgres, sqlite (fleet) or mysql (external).
        #[arg(long, default_value = "postgres")]
        engine: String,
        /// Who runs it: fleet (Stado, on a fleet host), supabase, or external (an existing postgres or mysql server; URL on stdin).
        #[arg(long, default_value = "fleet")]
        provider: String,
        /// Certificate authority bundle (PEM) of an external server (external).
        #[arg(long)]
        ca_certificate: Option<std::path::PathBuf>,
        /// Fleet host the database is placed on (fleet; default: the vault owner).
        #[arg(long)]
        host: Option<String>,
        /// Port the fleet database listens on (fleet postgres; default: the first free one from the engine's own).
        #[arg(long)]
        port: Option<u16>,
        /// Declared database whose project's organization and region the new project joins (supabase); without it, the one organization and region every project the token sees shares.
        #[arg(long)]
        anchor: Option<String>,
        /// Monthly compute cost in USD the operator accepts for the new project (supabase).
        #[arg(long)]
        accept_monthly_usd: Option<u64>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Place a fleet database on this host: the host-side half of `create
    /// --provider fleet`.
    ///
    /// Initialises the engine's data under `~/.stado/databases/<name>/`,
    /// writes the credential item `<name>-database` into the owner vault and,
    /// for postgres, installs the managed unit `<name>-database` that serves
    /// it over TLS with a certificate authority of its own. A database
    /// already placed here is reported, never initialised twice.
    Place {
        /// Logical database name (lowercase letters, digits, dashes).
        name: String,
        /// Engine the database speaks: postgres or sqlite.
        #[arg(long, default_value = "postgres")]
        engine: String,
        /// Port to listen on (postgres; default: the first free one from the engine's own).
        #[arg(long)]
        port: Option<u16>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Bring a declared database's credential item in line with its hosted
    /// Supabase project.
    ///
    /// Rewrites `<name>-database` from the management API: coordinates,
    /// pooler, the revealed API keys (legacy `anon_key` and
    /// `service_role_key`, every named key as `publishable_key_<name>` or
    /// `secret_key_<name>`) and an active custom hostname as `custom_url`.
    /// Fields another owner put on the item stay, and the password is kept
    /// from the item or read from --password-file. Without NAME, every
    /// declared database whose item records a Supabase `project_ref` is
    /// adopted again, so a rotated key lands. Runs on the owner vault host;
    /// --check writes nothing and exits non-zero when an item differs.
    Adopt {
        /// Declared database to adopt; every Supabase-backed one when omitted.
        name: Option<String>,
        /// The Supabase project ref, when the item does not record it yet.
        #[arg(long, requires = "name")]
        project_ref: Option<String>,
        /// File whose whole content is the database password.
        #[arg(long, requires = "name")]
        password_file: Option<String>,
        /// Report drift and write nothing; exit non-zero on drift.
        #[arg(long)]
        check: bool,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Make a host's database declarations equal to this machine's.
    ///
    /// Reads HOST's config file whole (its `config show` omits the block),
    /// writes this machine's `database_api` there through `host config-set`
    /// when the two differ, and reconciles --service so the running process
    /// reads it. --check writes nothing and exits non-zero on a difference.
    Push {
        host: String,
        /// The registry-managed unit on HOST that serves the database plane.
        #[arg(long)]
        service: String,
        /// Report the difference and write nothing; exit non-zero on it.
        #[arg(long)]
        check: bool,
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
    /// Destroy a fleet database: the inverse of `create --provider fleet`.
    ///
    /// Removes the managed unit `<name>-database` that serves a postgres
    /// database on its host, deletes the credential item from the owner vault
    /// (restorably, as Skarbiec deletes), and withdraws the declaration last,
    /// so a run that stops part-way can be run again. The data directory
    /// `~/.stado/databases/<name>/` on the host is left in place and named.
    Destroy {
        name: String,
        /// Host the database was placed on (default: the vault owner, where
        /// `create` places it).
        #[arg(long)]
        host: Option<String>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Grant one or more consumers access to a declared database.
    ///
    /// Adds each consumer to the declaration and widens that consumer's own
    /// Skarbiec grant (`~/.stado/<consumer>-skarbiec-token`) to read the
    /// database's credential item, keeping its bearer and every capability it
    /// already holds. Granting a consumer already on the list settles its
    /// Skarbiec read again; a consumer Skarbiec still refuses is named and the
    /// command exits non-zero.
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
