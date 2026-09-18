//! Where this host keeps the fleet's work: the one directory the job trees,
//! the build caches and the published free-space reading all hang from.
//!
//! Until 2026-09-18 that directory was the agent's home and nothing else,
//! spelled three times: the queue-workdir root walked `~/.stado/work/jobs`,
//! the release worker wrote its Cargo target under `~/.stado/build-cache`,
//! and the capacity gate measured free space at `~`. On a host whose home
//! sits on a small system volume all three were wrong together —
//! ubuntu-server-rtx-pro-6000 refused a 22 GiB build for want of room on
//! 98 GiB while 13 TiB sat free on `/mnt/wd16tb` — and there was no way to
//! tell the agent otherwise short of moving its home.
//!
//! The registry now declares `targets[].work_root`, and this module is the
//! one reader. The agent declares it into this process when it reads its
//! own target and hands it to every job it starts as [`ENV`], so the release
//! worker running inside a job answers the same directory the agent does.
//! A host that declares nothing keeps its home, exactly as before.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The environment variable a job process reads the declaration from. The
/// agent sets it on every job it starts; an operator may set it by hand
/// for a process that runs outside an agent.
pub const ENV: &str = "STADO_WORK_ROOT";

/// The queue-owned job tree root below a declared work root.
pub const JOBS_LEAF: &str = "jobs";

/// The persistent Cargo target cache below a declared work root, and the
/// same leaf below `~/.stado` when nothing is declared.
pub const BUILD_CACHE_LEAF: &str = "build-cache";

/// Components of the queue root below an undeclared host's home. Each is
/// opened separately with `O_DIRECTORY|O_NOFOLLOW`; no component symlink is
/// supported.
pub const HOME_JOBS_COMPONENTS: [&str; 3] = [".stado", "work", JOBS_LEAF];

static DECLARED: OnceLock<PathBuf> = OnceLock::new();

/// What one process was told about its work root, or nothing.
pub fn declared() -> Option<PathBuf> {
    if let Some(path) = DECLARED.get() {
        return Some(path.clone());
    }
    std::env::var_os(ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Declare the work root for this process. The first declaration wins: the
/// agent reads its target once per tick, and a root that changed under a
/// running agent would leave half the job trees on each side of it. An
/// agent that sees a different root declared while it has no running job
/// re-execs itself onto it ([`crate::self_update::reexec`]); with jobs
/// running it waits for them.
pub fn declare(path: &Path) -> bool {
    DECLARED.set(path.to_path_buf()).is_ok()
}

/// The registry's own rule for the declaration: an absolute, normalised
/// path of plain components, never `/` and never inside a system tree.
pub fn validate_declared(path: &str) -> Result<(), String> {
    let Some(relative) = path.strip_prefix('/') else {
        return Err("must be an absolute path such as /mnt/wd16tb/stado".to_string());
    };
    if relative.is_empty() || relative.ends_with('/') {
        return Err("must name a directory below /, not / itself".to_string());
    }
    let components: Vec<&str> = relative.split('/').collect();
    if components.iter().any(|component| {
        component.is_empty()
            || *component == "."
            || *component == ".."
            || !component
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.')
    }) {
        return Err(
            "must be a plain absolute path of letters, digits, '-', '_' and '.'".to_string(),
        );
    }
    if matches!(
        components[0],
        "boot" | "dev" | "etc" | "proc" | "run" | "sys" | "usr" | "bin" | "sbin" | "lib"
    ) {
        return Err(format!(
            "{path} is under a system tree; declare a directory on a data volume such as /mnt/<disk>/stado"
        ));
    }
    Ok(())
}

/// The directory the fleet's work hangs from and the queue root's components
/// below it: the declared root and `[jobs]`, or the resolved home and
/// [`HOME_JOBS_COMPONENTS`].
pub fn base_and_jobs_components() -> (PathBuf, &'static [&'static str]) {
    match declared() {
        Some(root) => (root, &[JOBS_LEAF]),
        None => {
            let home = crate::config_file::expand_tilde("~");
            (
                std::fs::canonicalize(&home).unwrap_or(home),
                &HOME_JOBS_COMPONENTS,
            )
        }
    }
}

/// The volume whose free space this host publishes and admits work against:
/// the declared root, or the home.
pub fn measured_volume() -> PathBuf {
    declared().unwrap_or_else(|| crate::config_file::expand_tilde("~"))
}

/// The persistent build cache root for one product and platform.
pub fn build_cache_root(home: &Path) -> PathBuf {
    match declared() {
        Some(root) => root.join(BUILD_CACHE_LEAF),
        None => home.join(".stado").join(BUILD_CACHE_LEAF),
    }
}
