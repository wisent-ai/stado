//! A fleet Postgres: initdb, a certificate authority of the database's own,
//! the credential item, and the managed unit that serves it.
//!
//! The item carries that authority as `ca_certificate` and the routable
//! address as `pooler_url`, exactly what `stado-database` verifies against,
//! so a consumer reaches a fleet database the way it reaches any other.

use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

use crate::cli::CmdError;

use super::owner_vault::Owner;
use crate::cli::database::dump::tools::program;

mod tls;

/// The role every fleet Postgres is initialised with.
const SUPERUSER: &str = "postgres";

pub(super) async fn place(
    name: &str,
    here: &str,
    directory: &Path,
    port: Option<u16>,
    version: Option<&str>,
    owner: &Owner,
) -> Result<Value, CmdError> {
    let data = directory.join("data");
    let unit = format!("{name}-database");
    let target = crate::cli::canonical_host(here).await?;
    let unit_installed = crate::deploy::service::declared_services(&target)
        .iter()
        .any(|service| service.matches(&unit));
    // A PG_VERSION that cannot be examined is refused: read as absent, an
    // initialised cluster would be treated as empty and initialised again.
    let version_file = data.join("PG_VERSION");
    if crate::primitives::presence::present(&version_file)? && version_file.is_file() {
        if !unit_installed {
            return Err(CmdError::click(format!(
                "{} holds an initialised database but {here} manages no unit {unit}: install it with \
                 `stado service deploy {unit} --host {here} --from <postgres> --arg -D --arg {}`",
                data.display(),
                data.display()
            ))
            .stating(crate::primitives::failure::FailureCode::Config));
        }
        return Ok(json!({
            "reused": true,
            "engine": "postgres",
            "host": here,
            "data": data.display().to_string(),
            "unit": unit,
        }));
    }
    let policy = crate::config::database_tls::PostgresTls::read().map_err(CmdError::declaration)?;
    let initdb = program("initdb", version)?;
    let postgres = program("postgres", version)?;
    let address = crate::cli::directory::routable_address(&target).ok_or_else(|| {
        CmdError::click(format!(
            "{here} declares no routable address (its ssh connection), so no consumer could reach a database placed on it"
        ))
        .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    // The port the caller states, else one this host hands out now on every
    // interface, as the server listens on every interface; it is written to
    // the configuration, the item and the report, so consumers read it there.
    // A handed-out port comes from the system's ephemeral range and is not
    // reserved, so the report says so and a unit that fails names it.
    let (port, handed) = match port {
        Some(port) => (port, false),
        None => (handed_out(name, here)?, true),
    };
    let password = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );

    let password_file = directory.join("initdb-password");
    write_private(&password_file, &password)?;
    let initialised = run(
        Command::new(&initdb)
            .arg("-D")
            .arg(&data)
            .arg("-U")
            .arg(SUPERUSER)
            .arg("--auth=scram-sha-256")
            .arg(format!("--pwfile={}", password_file.display())),
        "initdb",
    );
    let removed = std::fs::remove_file(&password_file);
    initialised?;
    removed.map_err(|error| {
        CmdError::click(format!("remove {}: {error}", password_file.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;

    let ca_certificate = tls::certificates(directory, &data, name, &address, &policy)?;
    append(
        &data.join("postgresql.conf"),
        &format!(
            "\n# Written by stado database place.\nlisten_addresses = '*'\nport = {port}\nssl = on\nssl_cert_file = 'server.crt'\nssl_key_file = 'server.key'\n"
        ),
    )?;
    append(
        &data.join("pg_hba.conf"),
        "\n# Written by stado database place: every remote connection is TLS with a password.\nhostssl all all all scram-sha-256\n",
    )?;

    // The password exists only in this process until the item holds it, so
    // the item is written before the unit can serve anything.
    let host_in_url = if address.contains(':') {
        format!("[{address}]")
    } else {
        address.clone()
    };
    // The server is reached directly, so its one URL keeps a server session
    // per connection: it is both `pooler_url` and the `session_url`
    // `stado_database::connect` opens.
    let url = format!("postgres://{SUPERUSER}:{password}@{host_in_url}:{port}/postgres");
    let fields = json!({
        "engine": "postgres",
        "provider": super::super::DatabaseProvider::Fleet,
        "host": here,
        "address": address,
        "port": port,
        "db_user": SUPERUSER,
        "db_password": password,
        "pooler_url": url,
        "session_url": url,
        "ca_certificate": ca_certificate,
    });
    let context = json!({
        "engine": "postgres",
        "provider": super::super::DatabaseProvider::Fleet,
        "product": name,
    });
    owner.store(&unit, "bundle", &fields, &context).await?;

    let data_text = data.display().to_string();
    let args = vec![
        "-D".to_string(),
        data_text.clone(),
        "-p".to_string(),
        port.to_string(),
    ];
    crate::cli::service::lifecycle::deploy::deploy(
        crate::cli::service::lifecycle::deploy::DeployOptions {
            name: &unit,
            host: Some(here),
            host_heuristic: None,
            from: Some(postgres.display().to_string()),
            from_artifact: None,
            args: &args,
            launchd_label: None,
            as_launch_agent: false,
            as_json: true,
        },
    )
    .await
    .map_err(|error| {
        let handed_note = match handed {
            true => format!(
                "; its port {port} was handed out by {here} from the ephemeral range and is not reserved, so \
                 another process may hold it (`lsof -nP -iTCP:{port}` on {here} names it); a placement whose port \
                 must not move states it with --port"
            ),
            false => String::new(),
        };
        let mut wrapped = CmdError::click(format!(
            "{unit} is stored and {data_text} is initialised, but the unit {unit} was not installed: {error}{handed_note}"
        ));
        wrapped.failure = error.failure;
        wrapped
    })?;
    Ok(json!({
        "reused": false,
        "engine": "postgres",
        "host": here,
        "address": address,
        "port": port,
        "port_handed_out": handed,
        "data": data_text,
        "unit": unit,
        "item": unit,
        "item_vault": owner.name(),
        "tls_policy": policy,
    }))
}

fn run(command: &mut Command, step: &str) -> Result<(), CmdError> {
    let output = crate::wait::output(command).map_err(|error| {
        CmdError::click(format!("{step} could not start: {error}"))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    if output.status.success() {
        return Ok(());
    }
    Err(CmdError::refused(format!(
        "{step} failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

fn write_private(path: &Path, text: &str) -> Result<(), CmdError> {
    std::fs::write(path, text).map_err(|error| {
        CmdError::click(format!("write {}: {error}", path.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    restrict(path)
}

fn restrict(path: &Path) -> Result<(), CmdError> {
    use std::os::unix::fs::PermissionsExt;
    let owner_only = crate::primitives::file_mode::widen(nix::libc::S_IRUSR | nix::libc::S_IWUSR);
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(owner_only))
        .map_err(|error| CmdError::from(error).within(format!("restrict {}", path.display())))
}

fn append(path: &Path, text: &str) -> Result<(), CmdError> {
    use std::io::Write;
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .map_err(|error| CmdError::from(error).within(format!("append to {}", path.display())))
}

/// A port this host hands out free on every interface, for the fleet
/// database `name`: the system chooses it, and the placement reports it.
fn handed_out(name: &str, here: &str) -> Result<u16, CmdError> {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, u16::default()))
        .map_err(|error| {
            CmdError::unreachable(format!(
                "{here} hands out no free port for the fleet database {name}: {error}; state one with --port"
            ))
        })?;
    let port = listener
        .local_addr()
        .map(|address| address.port())
        .map_err(|error| {
            CmdError::unreachable(format!(
                "the port {here} handed out for the fleet database {name} cannot be read back: {error}"
            ))
        })?;
    eprintln!(
        "{here}: no --port was stated for the fleet database {name}; the host handed out {port} from its ephemeral \
         range, which is not reserved for it"
    );
    Ok(port)
}
