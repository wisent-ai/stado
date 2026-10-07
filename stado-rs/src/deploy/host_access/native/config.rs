//! The SSH client settings a native session uses.
//!
//! Keepalive is the operator's to state, not Stado's: `ssh.keepalive_seconds`
//! (env `STADO_SSH_KEEPALIVE_SECONDS`) is how often an idle session asks the
//! server whether it is still there, and `ssh.keepalive_count_max` (env
//! `STADO_SSH_KEEPALIVE_COUNT_MAX`) how many unanswered asks end it. The 10 s
//! / 2 and 30 s / 3 pairs once written here were nobody's statement.
//!
//! A command session runs without keepalive when none is declared, as
//! OpenSSH does by default: it ends when its command does. A reverse forward
//! is long-lived and only keepalive tells it its peer is gone, so a forward is
//! refused, naming both keys, until they are declared. The port is SSH's
//! IANA-assigned one.

use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::config_file::resolve as cfg;

pub(super) const SSH_PORT: u16 = 22;
const KEEPALIVE_SECONDS: (&str, &str) = ("STADO_SSH_KEEPALIVE_SECONDS", "ssh.keepalive_seconds");
const KEEPALIVE_COUNT_MAX: (&str, &str) =
    ("STADO_SSH_KEEPALIVE_COUNT_MAX", "ssh.keepalive_count_max");

fn declared(key: (&str, &str)) -> Option<String> {
    let (env, name) = key;
    let value = cfg(env, name, "");
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

pub(super) fn client(forwarding: bool) -> Result<Arc<russh::client::Config>> {
    let interval = declared(KEEPALIVE_SECONDS);
    let count = declared(KEEPALIVE_COUNT_MAX);
    let (interval, count) = match (interval, count) {
        (Some(interval), Some(count)) => (interval, count),
        (None, None) if !forwarding => return Ok(Arc::new(russh::client::Config::default())),
        _ => bail!(
            "SSH keepalive is not declared: a {} needs both {} (env {}) and {} (env {}); \
             declare them with `stado config set`",
            if forwarding { "reverse forward" } else { "session with either key set" },
            KEEPALIVE_SECONDS.1,
            KEEPALIVE_SECONDS.0,
            KEEPALIVE_COUNT_MAX.1,
            KEEPALIVE_COUNT_MAX.0,
        ),
    };
    let seconds: NonZeroU64 = interval.parse().with_context(|| {
        format!("{} = {interval:?} is not a whole number of seconds above zero", KEEPALIVE_SECONDS.1)
    })?;
    let keepalive_max: usize = count.parse().with_context(|| {
        format!("{} = {count:?} is not a whole number of keepalives", KEEPALIVE_COUNT_MAX.1)
    })?;
    Ok(Arc::new(russh::client::Config {
        keepalive_interval: Some(Duration::from_secs(seconds.get())),
        keepalive_max,
        ..Default::default()
    }))
}
