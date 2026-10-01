//! `stado space volume mount|unmount`: give a disk a durable place in a
//! host's filesystem tree, and take it away again.

use clap::Subcommand;
use serde_json::Value;

use super::report::print_json;
use super::CmdError;

#[derive(Subcommand)]
pub enum SpaceVolumeCommands {
    /// Mount one block device at a mount point and write its fstab line by UUID.
    ///
    /// Mounts, never formats: a device with no filesystem is refused by
    /// name. `stado space report TARGET` lists the host's block devices and
    /// which of them nothing has mounted.
    Mount {
        target: String,
        /// The /dev leaf, such as sdb1 or nvme0n1p2.
        #[arg(long)]
        device: String,
        /// The absolute directory the filesystem is mounted at, such as /mnt/wd16tb.
        #[arg(long)]
        mount_point: String,
        #[arg(long)]
        json: bool,
    },
    /// Unmount the filesystem at a mount point and remove the fstab line
    /// `volume mount` wrote for it; the data on the disk is untouched.
    ///
    /// A busy filesystem is refused with umount's own error and fstab is left
    /// as it was. An fstab line stado did not write is refused by name. A
    /// mount point that is neither mounted nor declared is reported and
    /// nothing changes.
    Unmount {
        target: String,
        /// The absolute directory the filesystem is mounted at.
        #[arg(long)]
        mount_point: String,
        #[arg(long)]
        json: bool,
    },
}

pub(super) async fn dispatch(command: SpaceVolumeCommands) -> Result<(), CmdError> {
    match command {
        SpaceVolumeCommands::Mount {
            target,
            device,
            mount_point,
            json,
        } => super::ops::mount_volume(&target, &device, &mount_point, json).await,
        SpaceVolumeCommands::Unmount {
            target,
            mount_point,
            json,
        } => unmount(&target, &mount_point, json).await,
    }
}

async fn unmount(target: &str, mount_point: &str, json_output: bool) -> Result<(), CmdError> {
    use crate::deploy::host_volume::unmount;
    let runner = crate::deploy::production_runner();
    let (target, outcome) = unmount::unmount_volume(target, mount_point, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()).machine_readable(json_output))?;
    if json_output {
        print_json(&Value::Object(unmount::to_report(&target, mount_point, &outcome)))?;
    } else if outcome.error.is_none() {
        let what = if outcome.unmounted_now || outcome.fstab_removed {
            format!(
                "unmounted: {}; fstab line {}",
                if outcome.unmounted_now { "yes" } else { "was not mounted" },
                if outcome.fstab_removed { "removed" } else { "was not present" }
            )
        } else {
            "neither mounted nor declared; nothing changed".to_string()
        };
        println!("{}: {mount_point} {what}", target.name);
    }
    if let Some(error) = outcome.error {
        return Err(CmdError::click(format!("{}: {error}", target.name)).machine_readable(json_output));
    }
    Ok(())
}
