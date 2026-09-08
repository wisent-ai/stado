//! `stado stream` — declare, provision and operate an interactive session on a
//! fleet host, and say where to point the client.
//!
//! The question this answers is "use that machine's GPU from this laptop". A
//! board cannot be borrowed over a network, so the fleet renders on the host and
//! the client receives frames. Declaring it in the registry keeps the fleet's
//! own rule: the placement fact lives where every other placement fact lives.
//!
//! The parts, in the order an operator meets them: `command` is the clap
//! surface, `report` reads and prints what a host answered, `declare` writes the
//! declaration into the registry, and `operate` carries everything that reaches
//! a host — probe, apply, status, pair and stop.

use super::CmdError;

mod command;
mod declare;
mod operate;
mod report;

pub use command::StreamCommands;

use declare::declare;
use operate::{apply, pair, probe, status, stop};

pub async fn dispatch(command: StreamCommands) -> Result<(), CmdError> {
    match command {
        StreamCommands::Probe { target, json } => probe(&target, json).await,
        StreamCommands::Declare {
            target,
            resolution,
            refresh_hz,
            gpu_uuid,
            library_dir,
            steam,
            sunshine_url,
            sunshine_sha256,
            json,
        } => {
            declare(
                &target,
                &resolution,
                refresh_hz,
                gpu_uuid,
                &library_dir,
                steam,
                sunshine_url,
                sunshine_sha256,
                json,
            )
            .await
        }
        StreamCommands::Apply {
            target,
            provision_library,
            json,
        } => apply(&target, provision_library, json).await,
        StreamCommands::Status { target, json } => status(&target, json).await,
        StreamCommands::Pair {
            target,
            pin,
            client,
            json,
        } => pair(&target, &pin, &client, json).await,
        StreamCommands::Stop {
            target,
            purge,
            json,
        } => stop(&target, purge, json).await,
    }
}
