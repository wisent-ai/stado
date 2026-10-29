//! The SSH client settings a native session uses.
//!
//! Keepalive is the operator's to state, not Stado's: `ssh.keepalive_seconds`
//! (env `STADO_SSH_KEEPALIVE_SECONDS`) is how often an idle session asks the
//! server whether it is still there, and `ssh.keepalive_count_max` (env
//! `STADO_SSH_KEEPALIVE_COUNT_MAX`) how many unanswered asks end it. The 10 s
//! / 2 and 30 s / 3 pairs once written here were nobody's statement.
//!
//! A command session runs without keepalive when none is declared, as
//! OpenSSH does by default: it ends when its command does. A long-lived
//! session — a reverse forward, or the resolver's tunnel to its registry
//! authority, which opens channels on one session for as long as it lives —
//! learns its peer is gone only through keepalive, so it is refused, naming
//! both keys, until they are declared.

use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::config_file::resolve as cfg;

/// SSH's IANA-assigned port: https://www.iana.org/assignments/service-names-port-numbers/service-names-port-numbers.xhtml?search=ssh "ssh 22 tcp The Secure Shell (SSH) Protocol"
pub const SSH_PORT: u16 = 22;
const KEEPALIVE_SECONDS: (&str, &str) = ("STADO_SSH_KEEPALIVE_SECONDS", "ssh.keepalive_seconds");
const KEEPALIVE_COUNT_MAX: (&str, &str) =
    ("STADO_SSH_KEEPALIVE_COUNT_MAX", "ssh.keepalive_count_max");

fn declared(key: (&str, &str)) -> Option<String> {
    let (env, name) = key;
    let value = cfg(env, name, "");
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// The commands that declare keepalive, each key this host has not declared,
/// in the words every refusal of it uses.
fn declaring_commands() -> String {
    [KEEPALIVE_SECONDS, KEEPALIVE_COUNT_MAX]
        .into_iter()
        .filter(|key| declared(*key).is_none())
        .map(|(env, name)| {
            let value = if name == KEEPALIVE_SECONDS.1 {
                "SECONDS"
            } else {
                "COUNT"
            };
            format!("`stado config set {name} {value}` (or env {env})")
        })
        .collect::<Vec<_>>()
        .join(" and ")
}

/// Why this host's configuration cannot carry the long-lived sessions it
/// runs, or `None` when it can. A host runs one when a resolver has
/// published here: its tunnel to the registry authority lives as long as the
/// resolver. `stado config validate` reports it, so an incoming Stado
/// refuses to replace a binary on a host whose resolver it would stop.
pub(crate) fn long_lived_refusal() -> Option<String> {
    crate::cli::resolver::readiness_marker()?;
    let refused = client(true).err()?;
    Some(format!(
        "this host runs a resolver, whose tunnel to its registry authority is a long-lived SSH \
         session: {refused:#}"
    ))
}

/// The client settings of a session; `long_lived` is a session that outlives
/// any one command (a reverse forward, the resolver's authority tunnel).
pub(super) fn client(long_lived: bool) -> Result<Arc<russh::client::Config>> {
    let interval = declared(KEEPALIVE_SECONDS);
    let count = declared(KEEPALIVE_COUNT_MAX);
    let (interval, count) = match (interval, count) {
        (Some(interval), Some(count)) => (interval, count),
        (None, None) if !long_lived => return Ok(Arc::new(russh::client::Config::default())),
        _ => bail!(
            "SSH keepalive is not declared: a {} needs both {} and {}, since without them a \
             peer that stops answering never ends the session; declare it with {}",
            if long_lived {
                "long-lived session (a reverse forward or the resolver's authority tunnel)"
            } else {
                "session with either key set"
            },
            KEEPALIVE_SECONDS.1,
            KEEPALIVE_COUNT_MAX.1,
            declaring_commands(),
        ),
    };
    let seconds: NonZeroU64 = interval.parse().with_context(|| {
        format!(
            "{} = {interval:?} is not a whole number of seconds above zero",
            KEEPALIVE_SECONDS.1
        )
    })?;
    let keepalive_max: usize = count.parse().with_context(|| {
        format!(
            "{} = {count:?} is not a whole number of keepalives",
            KEEPALIVE_COUNT_MAX.1
        )
    })?;
    Ok(Arc::new(russh::client::Config {
        keepalive_interval: Some(Duration::from_secs(seconds.get())),
        keepalive_max,
        ..Default::default()
    }))
}
