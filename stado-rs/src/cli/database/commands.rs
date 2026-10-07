//! The `stado database` command surface: one clap subcommand per verb and
//! read, with the plane's contract carried in their help text.

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub(crate) enum DatabaseCommands {
    /// List declared databases and whether each is placed.
    ///
    /// A profile with no database declaration returns an empty list.
    /// A present malformed database section is refused, not reported as empty.
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
    /// Place a fleet database, create a Supabase project, or declare an existing external server.
    ///
    /// The default provider is `fleet`: Stado runs postgres (default) or
    /// sqlite (`--engine sqlite`) on one fleet host -- the vault owner unless
    /// `--host` names another -- with no vendor and no bill, through `stado
    /// database place` on that host. `supabase` creates a hosted Postgres
    /// project in the organization and region of `--anchor`'s project, or,
    /// without it, the one organization and region every project the token
    /// sees shares, and refuses unless `--accept-monthly-usd` covers what one
    /// more project adds to the compute bill. `external` brings a server of
    /// any engine the user already runs anywhere (Postgres, MySQL, MongoDB,
    /// Redis, SQL Server, ...; RDS, Cloud SQL, Neon, PlanetScale, Atlas,
    /// Azure, their own server): the connection URL is read from standard
    /// input and names the engine, and
    /// `--ca-certificate` names the server's certificate authority. Every way,
    /// the credential item `<name>-database` is written into the owner vault
    /// and the database is declared for its consumers. A fleet or supabase
    /// database of that name is reused, never duplicated; an external one is
    /// re-pointed at the URL given.
    ///
    /// The database name and consumer identities are checked before placement,
    /// provider requests or credential writes. An invalid name or consumer
    /// identity exits with code 2 and names the rejected value before any
    /// database is created or its credential item is rewritten.
    Create {
        /// Logical database name (lowercase letters, digits, dashes).
        name: String,
        /// Consumer allowed to resolve this database.
        #[arg(long = "consumer", value_delimiter = ',', required = true)]
        consumers: Vec<String>,
        /// Engine the database speaks. Fleet: postgres (default) or sqlite; supabase: postgres; external: the connection URL's engine, which --engine only confirms.
        #[arg(long)]
        engine: Option<String>,
        /// Who runs it: fleet (Stado, on a fleet host), supabase, or external (an existing server of any engine; URL on stdin).
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
    /// pooler with its transaction-mode `pooler_url` and session-mode
    /// `session_url`, the revealed API keys (legacy `anon_key` and
    /// `service_role_key`, every named key as `publishable_key_<name>` or
    /// `secret_key_<name>`) and an active custom hostname as `custom_url`.
    /// Fields another owner put on the item stay, and the password is kept
    /// from the item, read from --password-file, or, with --rotate-password,
    /// replaced: a new one is set on the project through the management API
    /// and written on the item, for a project whose password no item holds.
    /// Without NAME, every declared database whose item records a Supabase
    /// `project_ref` is adopted again, so a rotated key lands. Items are read
    /// and written in the owner vault, from any host: off the owner through
    /// the host channel. --check writes nothing and exits non-zero when an
    /// item differs.
    Adopt {
        /// Declared database to adopt; every Supabase-backed one when omitted.
        name: Option<String>,
        /// The Supabase project ref, when the item does not record it yet.
        #[arg(long, requires = "name")]
        project_ref: Option<String>,
        /// File whose whole content is the database password.
        #[arg(long, requires = "name")]
        password_file: Option<String>,
        /// Set a new database password on the Supabase project and store it
        /// on the item. Every client still using the old password loses its
        /// connection.
        #[arg(long, requires = "name", conflicts_with_all = ["password_file", "check"])]
        rotate_password: bool,
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
    /// Destroy a database Stado created: the inverse of `create`.
    ///
    /// A fleet database: removes the managed unit `<name>-database` that
    /// serves a postgres database on its host; its data directory
    /// `~/.stado/databases/<name>/` is left in place and named. A Supabase
    /// database: deletes the hosted project with every row it holds, and only
    /// with `--delete-project`. Then the credential item is deleted from the
    /// owner vault (restorably, as Skarbiec deletes) and the declaration is
    /// withdrawn last, so a run that stops part-way can be run again. An
    /// external database is a server Stado does not run: it is refused, and
    /// `remove` withdraws its declaration.
    ///
    /// On the vault owner, provider metadata is read with the owner's keys,
    /// without a workload bearer. Other hosts require access to the named
    /// credential item; hosted-provider API authorization is still required.
    Destroy {
        name: String,
        /// Host the database was placed on (default: the vault owner, where
        /// `create` places it).
        #[arg(long)]
        host: Option<String>,
        /// Required for a Supabase database: its hosted project and every row
        /// in it are deleted and cannot be restored.
        #[arg(long)]
        delete_project: bool,
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
    /// command exits non-zero. The result's `declaration_changed` field
    /// reports whether this command added a consumer, not whether Skarbiec
    /// granted access. On refusal, the error distinguishes a changed consumer
    /// declaration from an unchanged one. Consumers added before a refusal
    /// remain declared; retrying settles their credential access again.
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
    /// Let the database's library client read what `stado_database::connect` reads.
    ///
    /// A product opens its database through the `stado-database` crate as
    /// the Skarbiec consumer `<name>-database-client`. This widens that
    /// consumer's grant on the vault owner to the fields the crate reads:
    /// `pooler_url`, plus `ca_certificate` for a server and `session_url` for
    /// Postgres. The bearer in the owner's
    /// `~/.stado/<name>-database-client-skarbiec-token` is kept, so no host's
    /// copy needs syncing again. Running it again changes nothing. A client
    /// the vault owner holds no bearer for is refused with Skarbiec's answer.
    Client {
        name: String,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
}
