//! The caches this cleaner may never reclaim: the janitor's own state root,
//! the HuggingFace hub cache and the weles recordings root (each swept by its
//! own cleaner), cargo's package registry, and the running executable — plus,
//! on macOS, the personal locations the operating system puts behind a
//! consent dialog, which no walk of a home opens unless the host declares it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::providers::local::space_reclaim::STATE_DIR_PARTS;
use crate::targets::PRIVACY_FOLDERS;

/// Roots the build-cache cleaner must never delete even when they carry a
/// valid tag, plus (by prefix) everything beneath them.
///
/// This is not defensive decoration. Several tools tag `~/.cache` itself,
/// and `~/.cache/wisent-compute` holds the janitor's own state file and the
/// lock this very pass is holding open; `~/.cache/huggingface/hub` and the
/// weles recordings root belong to their own cleaners, which keep the
/// hub's blob references consistent and report recordings per run. A tag
/// file dropped one level above them would otherwise let this cleaner take
/// them as one opaque tree.
/// Cargo's package registry, which this cleaner would otherwise be entitled
/// to delete.
///
/// `~/.cargo/registry` carries a `CACHEDIR.TAG` whose signature is
/// byte-identical to [`CACHEDIR_SIGNATURE`] - cargo writes it itself - so the
/// walk below reads it as a build cache and may evict it. Everything else this
/// cleaner deletes is OUTPUT: a `target/` tree is reproduced by the next
/// build, from inputs already on the disk. The registry is INPUT, shared by
/// every build on the host, and it is not reproduced locally at all: it comes
/// back only by re-fetching from the network, and only if the network answers.
///
/// The failure that matters is not the lost bytes, it is the timing. Deleting
/// it under a running build removes source files that build scripts hold
/// absolute paths to: a release train's native build step dies exactly that
/// way - the crypto crate's build script reporting `no such file or
/// directory` for vendored C files inside this directory, then `ranlib`
/// unable to open the archive it had just written - on the one runner that
/// publishes every release.
///
/// `CARGO_HOME` is honoured because a build host may move it off the boot
/// volume, which is exactly the kind of host that arms a disk janitor.
///
/// [`CACHEDIR_SIGNATURE`]: crate::deploy::host_build_caches::CACHEDIR_SIGNATURE
fn cargo_home(home: &Path) -> PathBuf {
    match std::env::var_os("CARGO_HOME") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => home.join(".cargo"),
    }
}

/// Cargo's two input caches: the package `registry` and the `git` clones of
/// git dependencies. Both carry cargo's own `CACHEDIR.TAG`, and both are
/// INPUT for the reason given above. The git cache was not reserved: under
/// disk pressure the cleaner deleted `~/.cargo/git` while cargo was cloning
/// into it, and every build with a git dependency on the laptop (brama's
/// `cargo build`, `stado product update brama --surface desktop`) died with
/// `failed to create temporary file '~/.cargo/git/db/<repo>/objects/pack/…':
/// No such file or directory`, after re-fetching for as long as 24 minutes.
fn cargo_inputs(home: &Path) -> [PathBuf; 2] {
    let cargo = cargo_home(home);
    [cargo.join("registry"), cargo.join("git")]
}

pub(super) fn reserved_roots(home: &Path) -> Vec<PathBuf> {
    let mut roots = vec![
        home.join(STATE_DIR_PARTS[0]).join(STATE_DIR_PARTS[1]),
        home.join(".cache").join("huggingface").join("hub"),
        home.join("weles").join("recordings"),
    ];
    roots.extend(cargo_inputs(home));
    // A CLI running out of a tagged build tree must not unlink its own
    // executable while it is restoring the host's ability to do work.
    if let Ok(executable) = std::env::current_exe() {
        roots.push(executable);
    }
    roots
}

/// The home-relative top-level folders no walk of a home opens, for the
/// platform the walk runs on: on macOS the locations the operating system
/// puts behind a privacy decision, minus the consent-gated folders
/// (`Documents`, `Desktop`, `Downloads`) named in `admitted`.
///
/// Opening one of them is what makes macOS ask. `tccd` records the request
/// against the walking program and, when that program has no decision yet,
/// shows the person at the keyboard a dialog — "would like to access Apple
/// Music, your music and video activity, and your media library", "… your
/// photos", "… files in your Documents folder". An unattended pass has
/// nobody to answer it, a decision is kept per code identity, so every
/// differently signed build asks again, and a walk can win nothing there:
///
/// - `Pictures`, `Music` and `Movies` hold the Photos, Music and TV
///   libraries, bundles their own applications manage; no build tool tags
///   them with `CACHEDIR.TAG`.
/// - `Library` is the operating system's and every application's private
///   state: the Photos syndication library (`Library/Photos`), the media
///   library (`Library/iTunes`), Contacts, Calendars, Reminders, Mail,
///   Messages, Safari, other applications' containers, and the iCloud and
///   file-provider folders whose evicted entries download when read. No
///   build tool writes a tagged cache under it; the cleaners that own a root
///   there (the compiler cache's store, Playwright's browsers) open that root
///   by its declared path, never by walking.
/// - `.Trash` is the operator's undo, not build output.
/// - `Documents`, `Desktop` and `Downloads` are consent-gated and may hold
///   real build trees (this fleet keeps its checkouts under `~/Documents`).
///   A walk opens one only when the host's registry entry declares it in
///   `targets[].privacy_grants`: the operator then decided the fleet reads it,
///   and the one question macOS asks is the one he declared.
///
/// Every part is one top-level name below the home, so a walk refuses it by
/// comparing the first component; nothing below a part is ever opened.
///
/// This is the one list. The janitor's own walk reads it through
/// [`privacy_protected_roots`]; the build-cache verdict that `stado host
/// build-caches` and `stado host spotlight` run on a host, the census and
/// `du` inventory `stado space report` runs, and the `tagged_build_caches`
/// stage of `stado space reclaim` read it as the prune words their scripts
/// receive. A second list drifts: the janitor refusing a folder while a
/// report's `find` walks straight into it.
pub fn privacy_protected_parts(darwin: bool, admitted: &BTreeSet<String>) -> Vec<&'static str> {
    if !darwin {
        // No operating system outside macOS gates these directories behind a
        // consent dialog, and a Linux build host may legitimately keep a
        // tagged tree in any of them.
        return Vec::new();
    }
    let mut parts = vec!["Pictures", "Music", "Movies", ".Trash", "Library"];
    parts.extend(
        PRIVACY_FOLDERS
            .iter()
            .filter(|(key, _)| !admitted.contains(*key))
            .map(|(_, name)| *name),
    );
    parts
}

/// The refused roots under one home, for the platform this binary runs on,
/// with the consent-gated folders in `admitted` walked.
pub(super) fn privacy_protected_roots(home: &Path, admitted: &BTreeSet<String>) -> Vec<PathBuf> {
    privacy_protected_parts(cfg!(target_os = "macos"), admitted)
        .iter()
        .map(|part| home.join(part))
        .collect()
}
