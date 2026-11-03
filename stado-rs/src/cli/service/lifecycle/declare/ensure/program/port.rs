//! The port word a unit declaration writes where the service's port goes,
//! and the refusal of a unit written with it still in place.

use crate::cli::CmdError;

/// The word a catalog entry or a unit declaration writes where the service's
/// port goes.
pub(super) const PORT_WORD: &str = "$STADO_LISTEN_PORT";

/// `value` with the port word replaced by `listen_port`, when one is known.
pub(super) fn with_port(value: String, listen_port: Option<u16>) -> String {
    match listen_port {
        Some(port) => value.replace(PORT_WORD, &port.to_string()),
        None => value,
    }
}

/// Refuse a unit whose program, arguments or environment still carry the
/// port word. launchd and systemd expand no variables, so the word reached
/// the service as its text: Weles was started with
/// `WELES_API_PORT=$STADO_LISTEN_PORT` and listened on nothing it was probed
/// at.
pub(super) fn refuse_unresolved(
    host: &str,
    name: &str,
    program: &str,
    args: &[String],
    env: &[(String, String)],
) -> Result<(), CmdError> {
    let unresolved = std::iter::once(("its program", program))
        .chain(args.iter().map(|arg| ("an argument", arg.as_str())))
        .chain(env.iter().map(|(variable, value)| (variable.as_str(), value.as_str())))
        .find(|(_, word)| word.contains(PORT_WORD));
    let Some((place, _)) = unresolved else {
        return Ok(());
    };
    Err(CmdError::declaration(format!(
        "{host}: the unit of {name} takes its port as {PORT_WORD} in {place}, and the service \
         directory records no port for {name} on {host}; record its endpoint there (`stado \
         registry set --path service_directory.services.<service>.endpoints.{host}.url --value \
         http://127.0.0.1:<port>`), or let `service ensure` assign one, before the unit is \
         written"
    )))
}
