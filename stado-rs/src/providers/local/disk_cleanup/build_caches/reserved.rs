//! The caches this cleaner may never reclaim: the janitor's own state root,
//! the HuggingFace hub cache and the weles recordings root (each swept by its
//! own cleaner), cargo's package registry, and the running executable — plus,
//! on macOS, the personal locations the operating system puts behind a
//! consent dialog.

use std::path::{Path, PathBuf};

use crate::providers::local::disk_cleanup::STATE_DIR_PARTS;

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

/// The macOS locations a build tool never writes a tagged cache into, and
/// which cost a privacy prompt or a network download to look inside.
///
/// `$HOME` is this cleaner's root, so the walk reaches `~/Pictures`
/// like any other directory — and from an always-on agent it does: `tccd`
/// records `kTCCServicePhotos` requests attributed to
/// `~/.stado/bin/stado` while the pass was walking. macOS answers such a
/// request by asking the person at the keyboard, so an unattended cleaner
/// scanning a photo library produces a dialog nobody asked for, and produces
/// it again for every code identity that asks. The scan cannot win anything
/// there either: the media libraries are bundles their own applications
/// manage, and no build tool tags them with `CACHEDIR.TAG`.
///
/// `Library/Mobile Documents` and `Library/CloudStorage` are worse than
/// useless: their entries can be evicted placeholders, and reading one
/// downloads it. A disk cleaner that fills the disk to look for free space is
/// the opposite of the capability.
///
/// `Library/Group Containers` and `Library/Containers` are sandboxed
/// applications' own data. macOS gates reading another application's
/// container behind a consent prompt, and their trees carry iCloud-backed
/// content (Final Cut's `com.apple.CloudContent`, for one) whose every open
/// waits on the file provider: one pass spent over an hour opening those
/// directories, holding the cleanup lock while the signed release delivery
/// the disk-pressure rule admits waited behind it. No build tool writes a
/// tagged cache there. `Library/Developer/CoreSimulator` is the same kind of
/// tree: each simulated device's whole file system, which the next pass on
/// the same laptop spent its time opening instead.
///
/// `Documents`, `Desktop` and `Downloads` are deliberately NOT here. They are
/// consent-gated too, but real build trees live in them — this fleet's own
/// checkouts are under `~/Documents` — so the honest cost is one grant
/// decision for a stably signed binary, not a permanent blind spot.
///
/// The list is given as home-relative parts, for the platform a walk runs
/// on. This is the one list: the janitor's own walk reads it through
/// [`privacy_protected_roots`], and the build-cache verdict script that
/// `stado space report` sends to a host reads it through
/// `STADO_CACHE_PRUNE`. Two lists drift: the janitor refusing
/// `~/Library/CloudStorage` while the verdict's `find` walks straight into a
/// synced-drive `.tmp` and reports the whole host as `scan-failed`.
pub fn privacy_protected_parts(darwin: bool) -> &'static [&'static str] {
    if darwin {
        &[
            "Pictures",
            "Music",
            "Movies",
            ".Trash",
            "Library/Mobile Documents",
            "Library/CloudStorage",
            "Library/Group Containers",
            "Library/Containers",
            "Library/Developer/CoreSimulator",
        ]
    } else {
        // No operating system outside macOS gates these directories behind a
        // consent dialog, and a Linux build host may legitimately keep a
        // tagged tree in any of them.
        &[]
    }
}

/// The refused roots under one home, for the platform this binary runs on.
pub(super) fn privacy_protected_roots(home: &Path) -> Vec<PathBuf> {
    privacy_protected_parts(cfg!(target_os = "macos"))
        .iter()
        .map(|part| home.join(part))
        .collect()
}
