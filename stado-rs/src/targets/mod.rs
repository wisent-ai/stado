//! Registry models, validation, capability admission, and loading.
//!
//! The configured canonical backend supplies the target, service-directory,
//! placement and build declarations used by the fleet. Workload admission
//! compares [`Job`] requirements with those declared capabilities.
//! [`Registry::extra`] preserves top-level fields this reader does not model.
//!
//! [`fetch_registry_remote`] reads that authority with an in-process cache.
//! It returns [`RegistryFetchError`] instead of an empty registry when the
//! store cannot answer: an unavailable authority does not prove that a host
//! was removed and must not authorize the coordinator's rogue-daemon cleanup.
//!
//! Validated reads from a non-local store can update
//! `~/.stado/cache/registry-last-good.json`. [`fetch_registry_or_last_good`]
//! may serve that snapshot with [`Registry::staleness_seconds`] and a
//! diagnostic when the authority is unavailable. Local filesystem stores do
//! not populate or consume that cache.
//!
//! Only [`load_registry_auto`] can fall back further to the bundled empty
//! template. Authoritative reads do not use either fallback, and explicit
//! registry upload and validation commands never select the bundled template
//! as their input.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use chrono::{DateTime, SecondsFormat, Utc};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::models::Job;
use crate::queue::{BlobBackend, JobStorage, StorageError, VersionedText};

// One module, kept in parts small enough to read, grouped by what each part
// is about. Every leaf opens with `use crate::targets::*;`, so the imports
// above are the module's single import list and a part sees the items of
// every other part exactly as it did when this was one file. Each part is
// re-exported by glob: `pub` items stay public, `pub(crate)` items stay
// crate-visible, and the module's public surface is unchanged.

mod builds;
mod cache;
mod fleet;
mod lookup;
mod registry;
mod target;
mod validate;

pub use builds::*;
pub use cache::*;
pub use fleet::*;
pub use lookup::*;
pub use registry::*;
pub use target::*;
pub use validate::*;
