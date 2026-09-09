//! A `HOME` a test owns, carrying copies of the operator inputs a real-host
//! journey reads.
//!
//! Why this exists: the product records a last-known-good registry copy, its
//! resolver state and its scratch storage roots under `HOME`. A test that let
//! the built binary inherit the operator's home wrote all of that into the
//! operator's own `~/.stado` — `stado status` alone leaves
//! `~/.stado/cache/registry-last-good.json` behind. So `HOME` is always a
//! directory the test owns.
//!
//! A journey against a real fleet host still needs two things that live in the
//! operator's home: the ssh identity the host answers to, and the Stado
//! configuration naming the coordinator. Those are reads, and they are copied
//! in here rather than borrowed by pointing `HOME` at the operator.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Where the copied configuration lands: the third entry in the product's own
/// search order (`$STADO_CONFIG`, `./stado.config.json`,
/// `~/.config/stado/config.json`, `~/.stado/config.json`), so a command run
/// with this home resolves it without being told where to look.
const CONFIG_IN_HOME: &str = ".config/stado/config.json";

/// Copy the operator's ssh configuration and keys into `home`.
///
/// `std::fs::copy` carries the mode across, which ssh requires of a private
/// key, and the copies die with the caller's tempdir. Nothing is written back
/// to the operator's directory. A machine with no ssh directory cannot reach a
/// fleet host at all, and this says so instead of running on to a refusal
/// about the host.
pub fn copy_ssh_identity(home: &Path) {
    let source = operator_home().join(".ssh");
    let entries = std::fs::read_dir(&source).unwrap_or_else(|exc| {
        panic!(
            "a real-host journey authenticates with the operator's ssh identity at {}: {exc}",
            source.display()
        )
    });
    let destination = home.join(".ssh");
    std::fs::create_dir_all(&destination).expect("an ssh directory in the owned home");
    std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o700))
        .expect("ssh requires its directory to be private");
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_file()) {
            std::fs::copy(entry.path(), destination.join(entry.file_name()))
                .expect("copy one ssh input into the owned home");
        }
    }
}

/// Copy the operator's Stado configuration into `home`, and report where it
/// landed.
///
/// `None` means the operator has no configuration file at any searched path.
/// A command run with this home then resolves exactly the absent configuration
/// it would resolve at home, and refuses in its own words — which is the
/// honest state to be in, and is what the caller reports.
pub fn copy_stado_config(home: &Path) -> Option<PathBuf> {
    let source = operator_config()?;
    let destination = home.join(CONFIG_IN_HOME);
    std::fs::create_dir_all(destination.parent().expect("the copy has a directory"))
        .expect("a config directory in the owned home");
    std::fs::copy(&source, &destination).unwrap_or_else(|exc| {
        panic!(
            "copy the operator configuration {} into the owned home: {exc}",
            source.display()
        )
    });
    Some(destination)
}

/// The operator's configuration file, in the product's own search order.
fn operator_config() -> Option<PathBuf> {
    let named = std::env::var_os("STADO_CONFIG").map(PathBuf::from);
    let home = operator_home();
    [
        named,
        Some(PathBuf::from("stado.config.json")),
        Some(home.join(".config/stado/config.json")),
        Some(home.join(".stado/config.json")),
    ]
    .into_iter()
    .flatten()
    .find(|candidate| candidate.is_file())
}

fn operator_home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("the test process has a home"))
}
