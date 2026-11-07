//! Where this host keeps the fleet's work: the one directory the job trees,
//! the build caches and the published free-space reading all hang from.
//!
//! Without a declaration that directory is the agent's home, spelled three
//! times: the queue-workdir root walks `~/.stado/work/jobs`, the release
//! worker writes its Cargo target under `~/.stado/build-cache`, and the
//! capacity gate measures free space at `~`. On a host whose home sits on a
//! small system volume all three are wrong together — a build is refused for
//! want of room while terabytes sit free on another mount — and there was no
//! way to tell the agent otherwise short of moving its home.
//!
//! The registry now declares `targets[].work_root`, and this module is the
//! one reader. The agent declares it into this process when it reads its
//! own target and hands it to every job it starts as [`ENV`], so the release
//! worker running inside a job answers the same directory the agent does.
//! A host that declares nothing keeps its home, exactly as before.

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, OnceLock};

/// The environment variable a job process reads the declaration from. The
/// agent sets it on every job it starts; an operator may set it by hand
/// for a process that runs outside an agent.
pub const ENV: &str = "STADO_WORK_ROOT";

/// The queue-owned job tree root below a declared work root.
pub const JOBS_LEAF: &str = "jobs";

/// The persistent Cargo target cache below a declared work root, and the
/// same leaf below `~/.stado` when nothing is declared.
pub const BUILD_CACHE_LEAF: &str = "build-cache";

/// The session checkouts below a declared work root: one clone per
/// repository, `<work root>/session-checkouts/<repository>`, that an
/// `omp-session` works in. The janitor's `session_checkouts` cleaner takes
/// one the clone marker proves a session cloned, once no session works in it
/// and nothing in it is unpushed.
pub const SESSION_CHECKOUTS_LEAF: &str = "session-checkouts";

/// The clone marker: the file an `omp-session` writes into its checkout's
/// `.git` right after it cloned the repository there, holding the job's id.
/// It is the proof on which the janitor's `session_checkouts` cleaner takes
/// a checkout; a checkout without it was not cloned by a session (or by an
/// older release, whose queue record is then the proof).
pub const SESSION_CLONE_MARKER: &str = "stado-session-clone";

/// The Jeden session ledgers of the sessions the fleet runs on this host,
/// below a declared work root (`JEDEN_SESSION_ROOT` for every fleet session),
/// so a resumed session finds its ledger where the next one is written.
pub const JEDEN_SESSIONS_LEAF: &str = "jeden-sessions";

/// The download caches of the package managers a job runs, below a declared
/// work root, one directory per manager.
pub const PACKAGE_CACHES_LEAF: &str = "package-caches";

/// Each job's package managers, as the variable the manager reads its cache
/// directory from and that directory below [`PACKAGE_CACHES_LEAF`]: the
/// agent sets them on every job on a host that declares a work root, so a
/// job's installs are kept, shared and reclaimed on the fleet's volume.
pub const JOB_PACKAGE_CACHES: &[(&str, &str)] = &[
    ("PIP_CACHE_DIR", "pip"),
    ("UV_CACHE_DIR", "uv"),
    ("npm_config_cache", "npm"),
];

/// The work base below an undeclared host's home: what a work root holds
/// when one is declared, `~/.stado/work` holds when none is.
pub const HOME_WORK_COMPONENTS: &[&str] = &[".stado", "work"];

/// Components of the queue root below an undeclared host's home. Each is
/// opened separately with `O_DIRECTORY|O_NOFOLLOW`; no component symlink is
/// supported.
pub const HOME_JOBS_COMPONENTS: &[&str] = &[".stado", "work", JOBS_LEAF];

static DECLARED: OnceLock<PathBuf> = OnceLock::new();

/// What this host's last-known-good registry copy declares as its work
/// root, read once per process.
static RECORDED: LazyLock<Option<PathBuf>> = LazyLock::new(recorded);

/// What one process was told about its work root, or nothing: the agent's
/// own declaration, else [`ENV`], else this host's entry in the registry copy
/// every reader here falls back to. Without the last, a process the agent did
/// not start (`stado space reclaim --local`, `stado space candidates`, the
/// health publisher) judged and walked the home while the agent judged the
/// work root, so one host was called full by one and roomy by the other.
pub fn declared() -> Option<PathBuf> {
    if let Some(path) = DECLARED.get() {
        return Some(path.clone());
    }
    std::env::var_os(ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| RECORDED.clone())
}

/// This host's `targets[].work_root` in its last-known-good registry copy,
/// the document the agent refreshes on every registry read. A copy that is
/// absent declares nothing; one that cannot be read or does not resolve this
/// host is said once, and the home stays the work base.
fn recorded() -> Option<PathBuf> {
    let path = crate::targets::registry_last_good_path()?;
    if !path.exists() {
        return None;
    }
    let hostname = crate::providers::vast::system_hostname();
    let registry = match crate::targets::load_registry_file(&path) {
        Ok(registry) => registry,
        Err(error) => {
            eprintln!(
                "stado: the registry copy {} cannot be read, so this process keeps the fleet's \
                 work under the home: {error}",
                path.display()
            );
            return None;
        }
    };
    match registry.lookup_self(&hostname) {
        Ok(found) => found.and_then(|target| target.work_root.as_deref().map(PathBuf::from)),
        Err(error) => {
            eprintln!(
                "stado: {hostname} cannot be resolved in the registry copy {}, so this process \
                 keeps the fleet's work under the home: {error}",
                path.display()
            );
            None
        }
    }
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
        return Err("must be an absolute directory path on the selected data volume".to_string());
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
        components.first().copied(),
        Some("boot" | "dev" | "etc" | "proc" | "run" | "sys" | "usr" | "bin" | "sbin" | "lib")
    ) {
        return Err(format!(
            "{path} is under a system tree; declare a directory on a data volume such as /mnt/<disk>/stado"
        ));
    }
    Ok(())
}

/// The account's home ([`crate::config_file::home`]) as an I/O error when
/// neither HOME nor the user database names one, where this used to stop
/// the process.
fn home() -> std::io::Result<PathBuf> {
    crate::config_file::home().map_err(|cause| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("no work base: {cause}"),
        )
    })
}

/// The directory the fleet's work hangs from and the queue root's components
/// below it: the declared root and `[jobs]`, or the home and
/// [`HOME_JOBS_COMPONENTS`]. The base is returned as declared; every reader
/// resolves it (`queue_workdirs::roots`), and the one that opens it refuses a
/// base that cannot be resolved instead of taking the unresolved path.
pub fn base_and_jobs_components() -> std::io::Result<(PathBuf, &'static [&'static str])> {
    Ok(match declared() {
        Some(root) => (root, &[JOBS_LEAF]),
        None => (home()?, HOME_JOBS_COMPONENTS),
    })
}

/// The volume whose free space this host publishes and admits work against:
/// the declared root, or the home.
pub fn measured_volume() -> PathBuf {
    match declared() {
        Some(root) => root,
        None => crate::config_file::expand_tilde("~"),
    }
}

/// The persistent build cache root for one product and platform.
pub fn build_cache_root(home: &Path) -> PathBuf {
    match declared() {
        Some(root) => root.join(BUILD_CACHE_LEAF),
        None => home.join(".stado").join(BUILD_CACHE_LEAF),
    }
}
