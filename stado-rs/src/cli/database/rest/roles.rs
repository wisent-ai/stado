//! The Postgres roles PostgREST needs on the database it serves: the three a
//! request's key names, made `NOLOGIN` when a restored provider dump did not
//! already carry them, and the authenticator PostgREST logs in as, given a
//! fresh password and membership of the three.

use std::path::Path;

use crate::cli::CmdError;

use super::super::dump::restore::identifier;
use super::super::dump::tools::{query, Connection};

/// The role PostgREST connects as, then switches from.
pub(super) const AUTHENTICATOR: &str = "authenticator";
/// The roles a request's key may name, as the hosted provider names them;
/// `anon` is what a request without a key gets.
pub(super) const ANON: &str = "anon";
const AUTHENTICATED: &str = "authenticated";
pub(super) const SERVICE: &str = "service_role";

fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// A password or secret: two random UUIDs' hex, as fleet Postgres passwords
/// are made.
pub(super) fn generated() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Whether `role` exists on the database.
fn present(
    psql: &Path,
    connection: &Connection,
    ca: Option<&Path>,
    role: &str,
) -> Result<bool, CmdError> {
    let rows = query(
        psql,
        connection,
        ca,
        &format!(
            "select rolname from pg_roles where rolname = {}",
            literal(role)
        ),
        &format!("looking up role {role}"),
    )?;
    Ok(!rows.trim().is_empty())
}

/// Make every role PostgREST switches to, give the authenticator a new
/// password and the three roles, and answer that password.
pub(super) fn prepare(
    psql: &Path,
    connection: &Connection,
    ca: Option<&Path>,
) -> Result<String, CmdError> {
    for role in [ANON, AUTHENTICATED, SERVICE] {
        if !present(psql, connection, ca, role)? {
            query(
                psql,
                connection,
                ca,
                &format!("create role {} nologin", identifier(role)),
                &format!("creating role {role}"),
            )?;
        }
    }
    let password = generated();
    let verb = if present(psql, connection, ca, AUTHENTICATOR)? {
        "alter"
    } else {
        "create"
    };
    query(
        psql,
        connection,
        ca,
        &format!(
            "{verb} role {} login noinherit password {}",
            identifier(AUTHENTICATOR),
            literal(&password)
        ),
        "setting the authenticator role",
    )?;
    for role in [ANON, AUTHENTICATED, SERVICE] {
        query(
            psql,
            connection,
            ca,
            &format!(
                "grant {} to {}",
                identifier(role),
                identifier(AUTHENTICATOR)
            ),
            &format!("granting {role} to the authenticator"),
        )?;
    }
    Ok(password)
}
