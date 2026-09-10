//! Compute-target registry: data models, hostname validation, capability
//! admission, and local-file loading.
//!
//! Port of `stado/targets/__init__.py` (dataclasses + loader),
//! `stado/targets/validation.py` (registry-v2 contract + hostname
//! normalization), and `stado/targets/capabilities.py` (workload admission
//! against declared target capabilities — pure logic over the [`Job`]
//! model).
//!
//! The registry is the single source of truth for every box the queue can
//! route to: workstations, GCP zonal dispatchers, vast.ai pools. Like
//! Python, [`fetch_registry_remote`] fetches `registry.json` with a
//! short-TTL in-process cache and is the fleet-survival authority
//! (`source="gcs"`), while [`load_registry_auto`] adds the bundled file as
//! a fallback (`source="auto"`). On the "gcs" backend the fetch still goes
//! through the crate's GCS JSON-API backend, never gsutil — see the Python
//! `_load_from_gcs` docstring: a broken gsutil install knocked the agent
//! offline on 2026-05-08 even though the registry was in GCS.
//!
//! The same document carries the fleet's [`ServiceDirectory`] — which host
//! currently serves each service and which consumers may call it — and the
//! [`PlacementProfile`] groups that move those services between hosts.
//! [`Registry`] keeps every top-level key it does not model in `extra`, so a
//! writer built from this checkout cannot delete a block a newer publisher
//! added.
//!
//! DEVIATION from Python: the fetch follows `WC_STORAGE_BACKEND` instead of
//! hardcoding GCS. Python reads GCS unconditionally, so on an Azure-only
//! deployment the write side compare-and-swaps `registry.json` into the
//! Azure container (`cli::registry` — `stado registry push`, which already
//! goes through the configured store) while every reader consults a GCS
//! object nobody writes. The "gcs" read path is unchanged.
//!
//! [`fetch_registry_remote`] returns [`RegistryFetchError`] rather than an
//! empty registry, because "the store is unreachable" and "the registry
//! does not list you" drive opposite decisions in the coordinator's
//! rogue-daemon kill switch.
//!
//! A reader is not required to die with the authority. Every canonical read
//! that parses is copied to `~/.stado/cache/registry-last-good.json` with a
//! dated sidecar, and [`fetch_registry_or_last_good`] serves that copy —
//! carrying its age in [`Registry::staleness_seconds`] and one sentence for
//! the operator — when the store does not answer. The bundled snapshot stays
//! BELOW the cache, reachable only through [`load_registry_auto`]. What does
//! not change is the kill switch's authority: [`fetch_registry_remote`] still
//! fails rather than answer from a copy, because "the registry no longer
//! lists you" may only be concluded from the registry itself.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

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
