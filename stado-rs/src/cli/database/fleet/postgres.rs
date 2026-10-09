//! A fleet Postgres: initdb, a certificate authority of the database's own,
//! the credential item, and the managed unit that serves it.
//!
//! The item carries that authority as `ca_certificate` and the routable
//! address as `pooler_url`, exactly what `stado-database` verifies against,
//! so a consumer reaches a fleet database the way it reaches any other.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cli::CmdError;

use super::owner_vault::Owner;

mod tls;

/// The role every fleet Postgres is initialised with.
const SUPERUSER: &str = "postgres";

pub(super) async fn place(
    name: &str,
    here: &str,
    directory: &Path,
    port: Option<u16>,
    owner: &Owner,
) -> Result<Value, CmdError> {
    let data = directory.join("data");
    let unit = format!("{name}-database");
    let target = crate::cli::canonical_host(here).await?;
    let unit_installed = crate::deploy::service::declared_services(&target)
        .iter()
        .any(|service| service.matches(&unit));
    if data.join("PG_VERSION").is_file() {
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
    let initdb = program("initdb")?;
    let postgres = program("postgres")?;
    let address = crate::cli::directory::routable_address(&target).ok_or_else(|| {
        CmdError::click(format!(
            "{here} declares no routable address (its ssh connection), so no consumer could reach a database placed on it"
        ))
        .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    // Stado names no port: the placement's caller states the one the server
    // listens on.
    let port = port.ok_or_else(|| {
        CmdError::usage(format!(
            "placing the fleet database {name} on {here} needs --port: the port its Postgres \
             listens on; Stado assumes none"
        ))
    })?;
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
        "\n# Written by stado database place: every remote connection is TLS with a password.\nhostssl all all 0.0.0.0/0 scram-sha-256\nhostssl all all ::/0 scram-sha-256\n",
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
        "provider": "fleet",
        "host": here,
        "address": address,
        "port": port,
        "db_user": SUPERUSER,
        "db_password": password,
        "pooler_url": url,
        "session_url": url,
        "ca_certificate": ca_certificate,
    });
    let context = json!({ "engine": "postgres", "provider": "fleet", "product": name });
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
        let mut wrapped = CmdError::click(format!(
            "{unit} is stored and {data_text} is initialised, but the unit {unit} was not installed: {error}"
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
        "data": data_text,
        "unit": unit,
        "item": unit,
        "item_vault": owner.name(),
        "tls_policy": policy,
    }))
}

/// `name` where Stado resolves every program it starts on a host
/// ([`stado_product::common::step_program`]: its own installs, the toolchain
/// homes and Homebrew's, then this process's PATH), or a refusal naming where
/// it looked. `stado database place` runs over the host channel and inside the
/// host agent, whose PATH is minimal, so a lookup on that PATH alone refused a
/// host that has the server programs.
fn program(name: &str) -> Result<PathBuf, CmdError> {
    let found = stado_product::common::step_program(name);
    if found.is_absolute() && found.is_file() {
        return Ok(found);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            let searched = stado_product::common::step_search_path(None, &path)
                .unwrap_or_default();
            CmdError::click(format!(
                "{name} is not installed where Stado looks on this host ({}); install the engine's server programs here or place the database on a host that has them",
                searched.to_string_lossy()
            ))
            .stating(crate::primitives::failure::FailureCode::Config)
        })
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
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
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
