//! Issue the database authority and server certificate from the host's policy.

use std::{path::Path, process::Command};

use crate::{cli::CmdError, config::database_tls::PostgresTls};

use super::{program, restrict, run, write_private};

/// The database's own authority, and a server certificate it signs for the
/// routable address and loopback. Returns the authority's PEM, which is what
/// a consumer verifies the server against.
pub(super) fn certificates(
    directory: &Path,
    data: &Path,
    name: &str,
    address: &str,
    policy: &PostgresTls,
) -> Result<String, CmdError> {
    let openssl = program("openssl")?;
    let days = policy.certificate_days.to_string();
    let key = format!("rsa:{}", policy.rsa_bits);
    let authority_key = directory.join("ca.key");
    let authority = directory.join("ca.crt");
    run(
        Command::new(&openssl)
            .args(["req", "-x509", "-newkey", &key, "-nodes", "-days", &days])
            .arg("-subj")
            .arg(format!("/CN=stado database {name} authority"))
            .arg("-keyout")
            .arg(&authority_key)
            .arg("-out")
            .arg(&authority),
        "openssl req (authority)",
    )?;
    let request = directory.join("server.csr");
    let server_key = data.join("server.key");
    run(
        Command::new(&openssl)
            .args(["req", "-newkey", &key, "-nodes"])
            .arg("-subj")
            .arg(format!("/CN={address}"))
            .arg("-keyout")
            .arg(&server_key)
            .arg("-out")
            .arg(&request),
        "openssl req (server)",
    )?;
    let subject = if address.parse::<std::net::IpAddr>().is_ok() {
        format!("IP:{address}")
    } else {
        format!("DNS:{address}")
    };
    let extensions = directory.join("server.ext");
    write_private(
        &extensions,
        &format!(
            "basicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\nsubjectAltName={subject},IP:127.0.0.1,DNS:localhost\n"
        ),
    )?;
    run(
        Command::new(&openssl)
            .args(["x509", "-req", "-days", &days, "-CAcreateserial"])
            .arg("-in")
            .arg(&request)
            .arg("-CA")
            .arg(&authority)
            .arg("-CAkey")
            .arg(&authority_key)
            .arg("-extfile")
            .arg(&extensions)
            .arg("-out")
            .arg(data.join("server.crt")),
        "openssl x509 (sign server)",
    )?;
    for leftover in [&request, &extensions] {
        std::fs::remove_file(leftover).map_err(|error| {
            CmdError::click(format!("remove {}: {error}", leftover.display()))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?;
    }
    for key in [&authority_key, &server_key] {
        restrict(key)?;
    }
    std::fs::read_to_string(&authority).map_err(|error| {
        CmdError::click(format!("read {}: {error}", authority.display()))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })
}
