//! Chromium code-sign clone cleanup: eviction of the per-launch bundle clones
//! macOS leaves in this account's temporary container.
//!
//! macOS can leave a per-launch browser bundle clone after its process exits.
//! Those clones occupy the account's temporary container independently of
//! build caches and delivered worker trees. This cleaner identifies abandoned
//! clones without removing a bundle still used by a live browser.
//!
//! What may be taken is the clone of a launch that is over, and three gates
//! establish that, because macOS records nothing about which clone belongs to
//! which process:
//!
//! - **the policy's minimum age.** The clone is made at launch, so a browser
//!   that started within the retention window owns a clone younger than the
//!   gate. The registry's per-cleaner minimum in [`crate::targets`] applies.
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
