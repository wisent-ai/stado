//! Local publication inspection: no registry read, network probe or health claim.
use serde_json::json;

use super::{waiting_path, WaitingOpen};
use crate::cli::CmdError;

pub(in crate::cli::resolver) fn channels(
    bind: Option<&str>,
    consumer: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    let path = waiting_path().ok_or_else(|| {
        CmdError::declaration(
            "cannot locate resolver channel publication: neither STADO_RESOLVER_STATE_FILE nor HOME provides its path",
        )
    })?;
    let bytes = std::fs::read(&path).map_err(|error| {
        CmdError::from(error).within(format!(
            "read resolver channel publication {}",
            path.display()
        ))
    })?;
    let mut channels: Vec<WaitingOpen> = serde_json::from_slice(&bytes).map_err(|error| {
        CmdError::declaration(format!(
            "decode resolver channel publication {}: {error}",
            path.display()
        ))
    })?;
    channels.retain(|channel| {
        bind.is_none_or(|bind| channel.bind == bind)
            && consumer.is_none_or(|consumer| channel.consumer == consumer)
    });
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "source": path,
                "bind": bind,
                "consumer": consumer,
                "channels": channels,
                "health_observed": false,
            }))?
        );
    } else {
        println!(
            "resolver channel publication {} (no health probe)",
            path.display()
        );
        if channels.is_empty() {
            println!(
                "no matching published wait; this does not establish resolver or upstream health"
            );
        }
        for channel in channels {
            println!(
                "{} for {} at {}: {:?} to {} on {} since {}",
                channel.service,
                channel.consumer,
                channel.bind,
                channel.phase,
                channel.endpoint,
                channel.active_host,
                channel.since
            );
        }
    }
    Ok(())
}
