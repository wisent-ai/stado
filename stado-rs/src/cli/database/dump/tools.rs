//! The Postgres programs Stado starts for the database plane, of the major
//! version a server runs, and the libpq environment that points them at a
//! declared database's credential item.
//!
//! A dump must be taken by a `pg_dump` of the server's own major version, and
//! a fleet server that receives one must run that version, so a program is
//! looked up per major version: the Homebrew keg `postgresql@<major>` beside
//! every directory Stado's search knows, then the program the search finds
//! first when that one states the version asked for. Nothing here picks a
//! version for the caller.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

/// `name` (initdb, postgres, pg_dump, pg_restore, psql) of PostgreSQL
/// `major`, or, with no major, the one Stado's program search finds first.
pub(in crate::cli::database) fn program(
    name: &str,
    major: Option<&str>,
) -> Result<PathBuf, CmdError> {
    let Some(major) = major else {
        return searched_program(name);
    };
    let keg = format!("postgresql@{major}");
    let mut searched = Vec::new();
    let search = stado_product::common::step_search_path(None, None).map_err(|error| {
        CmdError::click(format!(
            "the directories Stado searches for {name} form no PATH: {error:#}"
        ))
        .stating(FailureCode::Config)
    })?;
    for directory in std::env::split_paths(&search) {
        if directory.file_name().is_none_or(|last| last != "bin") {
            continue;
        }
        let Some(prefix) = directory.parent() else {
            continue;
        };
        let candidate = prefix.join("opt").join(&keg).join("bin").join(name);
        if crate::primitives::presence::present(&candidate)? && candidate.is_file() {
            return Ok(candidate);
        }
        searched.push(candidate.display().to_string());
    }
    let found = searched_program(name)?;
    let found_major = major_of(&found)?;
    if found_major == major {
        return Ok(found);
    }
    Err(CmdError::click(format!(
        "PostgreSQL {major}'s {name} is needed here; {} is PostgreSQL {found_major} and none of {} exists: install the {keg} programs on this host",
        found.display(),
        searched.join(", ")
    ))
    .stating(FailureCode::Config))
}

/// `name` where Stado resolves every program it starts on a host
/// ([`stado_product::common::step_program`]: its own installs, the toolchain
/// homes and Homebrew's, then this process's PATH), or a refusal naming where
/// it looked. `stado database place` runs over the host channel and inside the
/// host agent, whose PATH is minimal, so a lookup on that PATH alone refused a
/// host that has the server programs.
fn searched_program(name: &str) -> Result<PathBuf, CmdError> {
    let path = std::env::var_os("PATH");
    stado_product::common::step_program(name, path.as_deref()).map_err(|error| {
        CmdError::click(format!(
            "{error:#}; install the engine's server programs here or place the database on a host that has them"
        ))
        .stating(FailureCode::Config)
    })
}

/// The major version `program --version` states (`pg_dump (PostgreSQL) 17.6`
/// is `17`), refused naming the program when it states none.
fn major_of(program: &Path) -> Result<String, CmdError> {
    let step = format!("{} --version", program.display());
    let text = output(Command::new(program).arg("--version"), &step)?;
    leading_major(&text).ok_or_else(|| {
        CmdError::click(format!("{step} printed {text:?}, which states no version"))
            .stating(FailureCode::Config)
    })
}

/// The digits that open the first word of `text` that opens with one: the
/// major version in `17.6 (Homebrew)` or `pg_dump (PostgreSQL) 17.6`.
fn leading_major(text: &str) -> Option<String> {
    let version = text
        .split_whitespace()
        .find(|word| word.starts_with(|c: char| c.is_ascii_digit()))?;
    version
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .filter(|digits| !digits.is_empty())
        .map(str::to_string)
}

/// Where a declared Postgres database is reached, read from its credential
/// item: the `session_url` `stado_database::connect` opens (one server
/// session per connection, which `pg_dump` and `pg_restore` need) without its
/// password, the item's `db_password`, and the certificate authority the
/// server is verified against. The declaration, not the URL, says the
/// database is Postgres; the caller checks it.
pub(in crate::cli::database) struct Connection {
    url: String,
    address: String,
    password: String,
    ca_certificate: Option<String>,
}

impl Connection {
    pub(in crate::cli::database) fn from_item(
        item: &str,
        fields: &Value,
    ) -> Result<Self, CmdError> {
        let refused =
            |what: String| CmdError::click(format!("{item} {what}")).stating(FailureCode::Config);
        let text = fields
            .get("session_url")
            .and_then(Value::as_str)
            .filter(|url| !url.is_empty())
            .ok_or_else(|| {
                refused("holds no session_url, so no Postgres session can be opened on it".into())
            })?;
        let mut url = url::Url::parse(text)
            .map_err(|error| refused(format!("session_url is not a URL: {error}")))?;
        let host = url
            .host_str()
            .filter(|host| !host.is_empty())
            .map(str::to_string);
        let port = url.port();
        let database = url.path().trim_start_matches('/').to_string();
        let (Some(host), Some(port)) = (host, port) else {
            return Err(refused("session_url names no host or no port".into()));
        };
        if url.username().is_empty() || database.is_empty() {
            return Err(refused("session_url names no user or no database".into()));
        }
        let address = format!("{host}:{port}/{database}");
        let password = fields
            .get("db_password")
            .and_then(Value::as_str)
            .filter(|password| !password.is_empty())
            .ok_or_else(|| refused("holds no db_password".into()))?
            .to_string();
        url.set_password(None)
            .map_err(|()| refused("session_url cannot have its password taken out".into()))?;
        let ca_certificate = fields
            .get("ca_certificate")
            .and_then(Value::as_str)
            .filter(|pem| !pem.trim().is_empty())
            .map(str::to_string);
        Ok(Self {
            url: url.to_string(),
            address,
            password,
            ca_certificate,
        })
    }

    pub(in crate::cli::database) fn ca_certificate(&self) -> Option<&str> {
        self.ca_certificate.as_deref()
    }

    /// `command` reaches this database through the session URL without its
    /// password (libpq decodes the URL itself) and the password through
    /// libpq's environment, so it never stands on a command line. A server
    /// whose item carries a certificate authority is verified against it
    /// (`ca_file`); one without is still reached over TLS. `flag` is how the
    /// program takes its connection (`--dbname`).
    pub(in crate::cli::database) fn apply(
        &self,
        command: &mut Command,
        flag: &str,
        ca_file: Option<&Path>,
    ) {
        for inherited in [
            "PGSERVICE",
            "PGSERVICEFILE",
            "PGPASSFILE",
            "PGOPTIONS",
            "PGSSLCERT",
            "PGSSLKEY",
        ] {
            command.env_remove(inherited);
        }
        command
            .env("PGPASSWORD", &self.password)
            .env("PGAPPNAME", "stado-database")
            .arg(format!("{flag}={}", self.url));
        match ca_file {
            Some(file) => command
                .env("PGSSLMODE", "verify-full")
                .env("PGSSLROOTCERT", file),
            None => command
                .env("PGSSLMODE", "require")
                .env_remove("PGSSLROOTCERT"),
        };
    }

    pub(in crate::cli::database) fn address(&self) -> &str {
        &self.address
    }
}

/// Run `command` and return its standard output, or refuse naming `step`,
/// its exit status and what it wrote on standard error.
pub(in crate::cli::database) fn output(
    command: &mut Command,
    step: &str,
) -> Result<String, CmdError> {
    let output = crate::wait::output(command).map_err(|error| {
        CmdError::click(format!("{step} could not start: {error}"))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    Err(CmdError::refused(format!(
        "{step} failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

/// One SQL statement through `psql` against `connection`, unaligned and
/// tuples-only, so each row is a line and each column is split by a tab.
pub(in crate::cli::database) fn query(
    psql: &Path,
    connection: &Connection,
    ca_file: Option<&Path>,
    sql: &str,
    step: &str,
) -> Result<String, CmdError> {
    let mut command = Command::new(psql);
    command.args([
        "-X",
        "-A",
        "-t",
        "-v",
        "ON_ERROR_STOP=on",
        "-F",
        "\t",
        "-c",
        sql,
    ]);
    connection.apply(&mut command, "--dbname", ca_file);
    output(&mut command, &format!("{step} on {}", connection.address()))
}

/// The server's major version (`server_version` `17.6` is `17`), read with
/// whichever `psql` Stado's search finds: any client reads this.
pub(in crate::cli::database) fn server_major(
    connection: &Connection,
    ca_file: Option<&Path>,
) -> Result<String, CmdError> {
    let psql = program("psql", None)?;
    let text = query(
        &psql,
        connection,
        ca_file,
        "show server_version",
        "reading the server version",
    )?;
    leading_major(&text).ok_or_else(|| {
        CmdError::click(format!(
            "{} answered server_version {text:?}, which opens with no version",
            connection.address()
        ))
        .stating(FailureCode::InfraDown)
    })
}
