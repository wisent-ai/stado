//! Every package cache the `package_caches` cleaner takes, and where each is
//! on this host.

use std::path::{Path, PathBuf};

use crate::providers::local::work_base;

/// Where one cache lives.
pub enum CacheRoot {
    /// Below the account's home.
    Home(&'static str),
    /// Cargo's git dependency checkouts, below Cargo's home.
    CargoGit,
    /// The download caches of the package managers the fleet's jobs run,
    /// below the host's declared work root ([`work_base::JOB_PACKAGE_CACHES`]);
    /// a host that declares none has none.
    JobCaches,
}

/// One cache: where it is and which tool keeps it.
pub struct PackageCache {
    pub root: CacheRoot,
    pub tool: &'static str,
}

/// Every cache this cleaner takes. A product's own application cache
/// (`~/Library/Caches/<bundle id>` of an app the owner uses) is not one: it
/// is the owner's, and `stado space candidates` only proposes it.
pub const CACHES: &[PackageCache] = &[
    PackageCache {
        root: CacheRoot::CargoGit,
        tool: "Cargo git dependency checkouts",
    },
    PackageCache {
        root: CacheRoot::Home(".npm/_cacache"),
        tool: "npm package cache",
    },
    PackageCache {
        root: CacheRoot::Home(".bun/install/cache"),
        tool: "Bun package cache",
    },
    PackageCache {
        root: CacheRoot::Home("Library/Caches/org.swift.swiftpm"),
        tool: "SwiftPM package cache",
    },
    PackageCache {
        root: CacheRoot::Home("Library/Caches/pip"),
        tool: "pip package cache",
    },
    PackageCache {
        root: CacheRoot::Home(".cache/pip"),
        tool: "pip package cache",
    },
    PackageCache {
        root: CacheRoot::Home(".cache/uv"),
        tool: "uv package cache",
    },
    PackageCache {
        root: CacheRoot::Home("Library/Caches/Homebrew"),
        tool: "Homebrew download cache",
    },
    PackageCache {
        root: CacheRoot::Home("Library/Caches/com.microsoft.VSCode.ShipIt"),
        tool: "VS Code update staging",
    },
    PackageCache {
        root: CacheRoot::JobCaches,
        tool: "package caches of the fleet's jobs (pip, uv, npm) on the work root",
    },
];

/// Cargo's home: `CARGO_HOME`, else `~/.cargo`.
pub(super) fn cargo_home(home: &Path) -> PathBuf {
    match std::env::var_os("CARGO_HOME") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home.join(".cargo"),
    }
}

/// Where `cache` is on this host, or nothing for the jobs' caches of a host
/// that declares no work root. Of Cargo's git cache only `checkouts/` goes:
/// each is a working tree Cargo checks out again from `db/`, the fetched
/// repositories, which stay.
pub fn cache_path(home: &Path, cache: &PackageCache) -> Option<PathBuf> {
    match cache.root {
        CacheRoot::Home(root) => Some(home.join(root)),
        CacheRoot::CargoGit => Some(cargo_home(home).join("git").join("checkouts")),
        CacheRoot::JobCaches => {
            work_base::declared().map(|root| root.join(work_base::PACKAGE_CACHES_LEAF))
        }
    }
}
