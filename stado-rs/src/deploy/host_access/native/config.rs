//! Native equivalents of the existing resolver and reverse-forward SSH settings.
//! These transport constants preserve existing configuration, not new tuning.

use std::sync::Arc;
use std::time::Duration;

pub(super) const SSH_PORT: u16 = 22;
const SERVER_ALIVE_INTERVAL: Duration = Duration::from_secs(10);
const SERVER_ALIVE_COUNT_MAX: usize = 2;
const FORWARD_ALIVE_INTERVAL: Duration = Duration::from_secs(30);
const FORWARD_ALIVE_COUNT_MAX: usize = 3;

pub(super) fn client(forwarding: bool) -> Arc<russh::client::Config> {
    let (interval, count) = if forwarding {
        (FORWARD_ALIVE_INTERVAL, FORWARD_ALIVE_COUNT_MAX)
    } else {
        (SERVER_ALIVE_INTERVAL, SERVER_ALIVE_COUNT_MAX)
    };
    Arc::new(russh::client::Config {
        keepalive_interval: Some(interval),
        keepalive_max: count,
        ..Default::default()
    })
}
