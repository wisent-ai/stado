//! Activate a release that is already staged on a host, using that release's
//! own installer.
//!
//! A managed host installs its own releases: a periodic unit runs the installer
//! that ships INSIDE the active release, which reads the deployment env file,
//! verifies the staged archive against the digest that file declares, unpacks
//! it and points the runtime at it. That works until the installer itself is
//! broken, and then it cannot be repaired by shipping a better one, because
//! the thing that would install the repair is the broken copy.
//!
//! charless-mac-mini spent a day in exactly that state: weles 0.5.40 shipped
//! `auto-deploy.sh` with a blank line inside a backslash continuation, so its
//! activator logged `syntax error near unexpected token '&&'` once per cycle
//! and installed nothing - including 0.5.43, which fixes that line and was
//! sitting in its local release root the whole time.
//!
//! This runs the STAGED archive's installer instead of the installed one, once.
//! Same env file, same digest contract, same script the host would have run
//! itself; the only difference is which copy of it executes. Two refusals guard
//! it: the staged archive must hash to the digest the coordinate declares, and
//! the installer must parse before it is run - the exact defect this exists to
//! escape should not be able to travel through it.
//!
//! The four steps of that sentence are the four components beside this file:
//! what the env file declares in `declaration`, the archive path it names in
//! `archive`, the digest contract in `digest`, and the activation itself - the
//! program the host runs, plus the readings that bracket it - in `activation`.
//! The two keys both a declaration and an activation name stay here.
use super::{host_channel, service_env_file, shlex_quote, DeployError, Runner};
use crate::targets::ComputeTarget;

mod activation;
mod archive;
mod declaration;
mod digest;

pub use activation::*;
pub use archive::*;
pub use declaration::*;
pub use digest::*;

/// Where a host keeps staged release archives when the deployment env file
/// selects a local root rather than an API.
pub const LOCAL_ROOT_KEY: &str = "STADO_RELEASE_LOCAL_ROOT";

/// The installer every Weles release ships, relative to the archive root.
pub const INSTALLER_MEMBER: &str = "scripts/worker/deploy/auto-deploy.sh";
