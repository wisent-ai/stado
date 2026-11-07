//! A second real APFS volume beside the isolated home: the data disk a host
//! declares as its work root, so a journey can fill the home's volume and
//! leave the fleet's volume roomy, as a fleet host with a small system disk
//! and a large data disk is.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::super::Native;

/// The data volume's size: the same as the home's, so either one is only
/// full when a journey fills it.
const VOLUME_MEGABYTES: u64 = super::super::VOLUME_MEGABYTES;

/// A mounted data volume, detached and its image removed when dropped.
pub struct DataVolume {
    pub mountpoint: PathBuf,
    image: PathBuf,
}

impl Native {
    /// Attach an empty APFS volume named `name` beside the home and return
    /// it; nothing the journey does to the home's volume touches it.
    pub fn data_volume(&self, name: &str) -> DataVolume {
        let image = self.run.join(format!("{name}.sparseimage"));
        let mountpoint = self.run.join(name);
        std::fs::create_dir(&mountpoint).unwrap();
        let size = format!("{VOLUME_MEGABYTES}m");
        let volume_name = format!("stado-data-{name}");
        self.tool(
            "/usr/bin/hdiutil",
            &[
                "create",
                "-quiet",
                "-size",
                &size,
                "-fs",
                "APFS",
                "-type",
                "SPARSE",
                "-volname",
                &volume_name,
                image.to_str().unwrap(),
            ],
        );
        self.tool(
            "/usr/bin/hdiutil",
            &[
                "attach",
                "-quiet",
                "-nobrowse",
                "-owners",
                "on",
                "-mountpoint",
                mountpoint.to_str().unwrap(),
                image.to_str().unwrap(),
            ],
        );
        DataVolume { mountpoint, image }
    }
}

impl DataVolume {
    pub fn path(&self) -> &Path {
        &self.mountpoint
    }
}

impl Drop for DataVolume {
    fn drop(&mut self) {
        let detached = Command::new("/usr/bin/hdiutil")
            .args(["detach", "-quiet", "-force", self.mountpoint.to_str().unwrap()])
            .status()
            .is_ok_and(|status| status.success());
        if detached {
            if let Err(error) = std::fs::remove_file(&self.image) {
                eprintln!("cannot remove {}: {error}", self.image.display());
            }
        } else {
            eprintln!("cannot detach the data volume at {}", self.mountpoint.display());
        }
    }
}
