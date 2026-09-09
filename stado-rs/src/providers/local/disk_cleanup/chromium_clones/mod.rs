//! Chromium code-sign clone cleanup: eviction of the per-launch bundle clones
//! macOS leaves in this account's temporary container.
//!
//! NO Python original. Measured on `control-host` on 2026-08-18: free
//! space had fallen to about 2 GiB against the registry's 55 GiB policy, its
//! queue agent published `disk_pressure_unresolved`, admission failed closed,
//! and every release build queued behind that host for hours. Three consumers
//! held the space. Two of them are now stages of
//! [`crate::deploy::host_reclaim`] — `$HOME/.stado/build-work` at about 21 GiB
//! and the legacy delivered worker trees at about 9 GiB. The third had no
//! owner anywhere in the product: `<temporary container>/`[`CLONE_CONTAINER`]
//! `/`[`CLONE_ROOT_NAME`], where macOS clones the entire browser bundle on
//! EVERY launch so it can validate a signature against an object nobody can
//! swap underneath it. Weles drives Chromium for browser automation, so that
//! host launches it constantly, and a run that is killed leaves its clone
//! behind:
//! 137 of them on the mini when this was written, 130 untouched for
//! more than a day, and neither the janitor nor any command removed or even
//! reported a single one.
//!
//! What may be taken is the clone of a launch that is over, and three gates
//! establish that, because macOS records nothing about which clone belongs to
//! which process:
//!
//! - **the policy's minimum age.** The clone is made at launch, so a browser
//!   that started within the retention window owns a clone younger than the
//!   gate. The registry floors this cleaner at a day
//!   ([`crate::targets`]'s per-cleaner minimum), which is the same floor the
//!   weles and build-cache cleaners carry and the same one the shell script
//!   written during the outage used.
//! - **the newest clone, kept unconditionally.** A browser that has been up
//!   longer than the retention window has a clone older than the gate, and it
//!   is the most recent one in the root: keeping it costs one bundle and
//!   removes the only case age alone cannot see. Same rule, same reason, as
//!   `space reclaim`'s "never the newest artefact".
//! - **one snapshot of the process table per pass.** A clone whose path any
//!   live argv names is never a candidate — that is what an app launched out
//!   of its own clone (a translocated bundle) looks like from outside.
//!
//! Every operation is path-based, as in [`super::weles`] rather than
//! [`super::hf`]: the root is a fixed, owner-only directory of shallow entries
//! macOS itself named, so the ordered refusals below plus the parent check are
//! the safety gate, and the tree walk and the removal are that module's —
//! imported, not copied.
//!
//! `expected_bytes` here is apparent size, and for clones it OVERSTATES what
//! comes back: macOS makes them with `clonefile`, so a clone shares its blocks
//! with the installed bundle until one of them is written to. The number that
//! is true is `actual_free_delta_bytes`, measured either side of each removal
//! the same way every other cleaner measures it.
//!
//! Components: `names` the registry name and the three fixed macOS names,
//! `root` the container resolution, `processes` the one process-table
//! snapshot, `scan` the pass itself.

mod names;
mod processes;
mod root;
mod scan;

pub use names::{CLEANER, CLONE_CONTAINER, CLONE_ENTRY_PREFIX, CLONE_ROOT_NAME};
pub use root::default_root;
pub use scan::scan_chromium_clones;
