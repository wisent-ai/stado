//! Whether this host has memory left for new work: its swap use against the
//! ceiling the operator declared (`host.swap_ceiling_pct`).
//!
//! A host whose swap is nearly full does not refuse work, it pages: a Stado
//! build that takes minutes took over half an hour with its compiler at two
//! percent of a core, and every installation queued behind it. No ceiling is
//! assumed: a host that declares none has none, and its swap is only read.

use crate::capabilities::HOST_SWAP_CEILING_PCT_CONFIG;
use crate::providers::local::host_memory::read_host_memory;

/// The ceiling declared for this host, `None` when nothing is declared.
pub fn ceiling() -> Result<Option<i64>, String> {
    let field = &HOST_SWAP_CEILING_PCT_CONFIG;
    let declared = crate::config_file::resolve(field.env, field.path, "");
    let declared = declared.trim();
    if declared.is_empty() {
        return Ok(None);
    }
    match declared.parse::<i64>() {
        Ok(value) if (1..=100).contains(&value) => Ok(Some(value)),
        _ => Err(format!(
            "{} = {declared:?} is not a percentage from 1 to 100",
            field.path
        )),
    }
}

/// Why new work must wait now: swap used above the declared ceiling, said as
/// the reading; `None` when it may start (no ceiling, swap under it, or swap
/// that cannot be read, whose cause is returned beside).
pub fn over_ceiling() -> Result<(Option<String>, Option<String>), String> {
    let Some(ceiling) = ceiling()? else {
        return Ok((None, None));
    };
    let reading = read_host_memory();
    let Some(used) = reading.swap_used_pct() else {
        return Ok((
            None,
            Some(format!(
                "swap could not be read, so {} {ceiling} was not applied: {}",
                HOST_SWAP_CEILING_PCT_CONFIG.path,
                reading.unread.as_deref().unwrap_or("no swap is configured")
            )),
        ));
    };
    if used <= ceiling {
        return Ok((None, None));
    }
    let total = reading
        .swap_total_bytes
        .map(|bytes| format!("{} MiB", bytes / (1024 * 1024)))
        .unwrap_or_else(|| "an unread total".to_string());
    Ok((
        Some(format!(
            "swap {used}% of {total} used, above {} {ceiling}",
            HOST_SWAP_CEILING_PCT_CONFIG.path
        )),
        None,
    ))
}

/// How often a waiting operation reads swap again: the kernel's swap use
/// moves as pages are freed, and a minute is the load average's own window.
const REREAD: std::time::Duration = std::time::Duration::from_secs(60);

/// Wait, as a `czekam:` host wait naming the reading, until this host's swap
/// is under its declared ceiling, then return; at once when no ceiling is
/// declared or swap is under it. `what` names the operation that waits. A
/// swap that cannot be read admits the operation and says why on stderr.
pub async fn wait_under_ceiling(what: &str) -> Result<(), String> {
    let (mut full, unread) = over_ceiling()?;
    if let Some(unread) = unread {
        eprintln!("{what}: {unread}");
    }
    let Some(first) = full.clone() else {
        return Ok(());
    };
    let waiting = crate::wait::begin(
        crate::wait::Kind::Host,
        format!("{what}: this host's swap to fall under its ceiling ({first})"),
        "this host's memory",
    );
    while full.is_some() {
        tokio::time::sleep(REREAD).await;
        full = match over_ceiling() {
            Ok((full, _)) => full,
            Err(error) => {
                waiting.failed(&error);
                return Err(error);
            }
        };
    }
    waiting.done();
    Ok(())
}