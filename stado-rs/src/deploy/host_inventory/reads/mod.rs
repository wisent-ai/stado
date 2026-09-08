//! The remote program, and the typed records one host answers with.
//!
//! One component per thing the inventory reads: `filesystem` for the lstat
//! metadata and the Skarbiec vault files, `software` for the installed
//! program products and the subcommand probe, `caches` for Cargo, and
//! `services` for the service artefacts, the forward markers and the socket
//! table. The shell section held beside each of those components is one part
//! of the single program below.

use crate::deploy::{products, shlex_quote, DeployError};

mod caches;
mod filesystem;
mod services;
mod software;

pub(super) use caches::clamp_cargo_inventory;
pub(super) use filesystem::{clamp, clamp_vault_section};

pub use caches::CargoInventory;
pub use filesystem::{FilesystemMetadata, VaultFile};
pub use services::{
    declaration_verdict, declared_adapter, declared_endpoint, marker_port, verdict, ForwardMarker,
    Listener, ServiceArtifact,
};
pub use software::{reported_version, version_verdict, ManagedBinary, Subcommand};

/// The remote program.
///
/// Nothing an operator says is interpolated into it. The one value bound in
/// front of it is the declared program set
/// ([`crate::deploy::products::installed_programs`]), which
/// [`remote_inventory_script`] quotes as a single newline-delimited
/// assignment; every expansion below is quoted, every value passes through
/// `sanitize`, and every external program is named by absolute path the way
/// the recovery and GUI-automation scripts name theirs.
///
/// The program text is held beside this file as the ordered shell sections
/// the components below read, concatenated here in the order the report
/// emits them.
pub const REMOTE_INVENTORY_BODY: &str = concat!(
    include_str!("filesystem/stat.sh"),
    include_str!("software/binaries.sh"),
    include_str!("services/artifacts.sh"),
    include_str!("caches/cargo.sh"),
    include_str!("services/forwards.sh"),
    include_str!("services/listeners.sh"),
    include_str!("software/subcommands.sh"),
    include_str!("filesystem/vaults.sh"),
);

/// The remote program, bound to the program products this fleet declares.
///
/// The loop that reads `$HOME/.stado/bin` used to spell `for binary_name in
/// stado skarbiec` into the program text, and to ask `[ "$binary_name" =
/// stado ]` which version argument to send. Both facts are declared
/// ([`crate::deploy::products`]), and both are now read from one quoted
/// tab-separated binding, so the command that REPORTS what a host runs and
/// the command that DELIVERS it cannot disagree about which programs exist or
/// how to ask one its version.
pub fn remote_inventory_script() -> Result<String, DeployError> {
    let mut rows = String::new();
    for (name, root, argument, shape) in products::installed_programs()? {
        // The root is `$HOME`-relative in the declaration and stays that way
        // on the wire: only the host knows what `$HOME` is, and expanding it
        // here would bind one host's answer into every host's program.
        let relative = root.strip_prefix("$HOME/").unwrap_or(root);
        rows.push_str(&format!("{name}\t{relative}\t{argument}\t{shape}\n"));
    }
    Ok(format!(
        "managed_programs={}\n{REMOTE_INVENTORY_BODY}",
        shlex_quote(rows.trim_end())
    ))
}
