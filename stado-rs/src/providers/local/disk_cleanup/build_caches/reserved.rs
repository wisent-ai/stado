//! The caches this cleaner may never reclaim: the janitor's own state root,
//! the HuggingFace hub cache and the weles recordings root (both owned by
//! stricter cleaners), cargo's package registry, and the running executable —
//! plus, on macOS, the personal locations the operating system puts behind a
//! consent dialog.

use std::path::{Path, PathBuf};

use crate::providers::local::disk_cleanup::STATE_DIR_PARTS;
use crate::targets::DiskCleanupPolicy;

/// Roots the build-cache cleaner must never delete even when they carry a
/// valid tag, plus (by prefix) everything beneath them.
///
/// This is not defensive decoration. Several tools tag `~/.cache` itself,
/// and `~/.cache/wisent-compute` holds the janitor's own state file and the
/// lock this very pass is holding open; `~/.cache/huggingface/hub` and the
/// weles recordings root belong to cleaners whose deletion rules are
/// stricter than a tag file (blob reference counts, durable upload proof).
/// A single tag file dropped one level above them would otherwise let the
/// youngest cleaner in the janitor overrule both of the older ones.
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
/// absolute paths to. On 2026-08-31 at 19:15Z the `stado-v0.13.14` train's
/// `Build native Rust control plane` step died exactly that way -
/// `aws-lc-sys` reporting `no such file or directory` for two vendored C files
/// inside this directory, then `ranlib` unable to open the archive it had just
/// written - on the one runner that publishes every release. That extraction
/// verified complete afterwards (2010 of 2010 files), so that particular
/// failure was transient rather than this cleaner's work; the point is that
/// this cleaner was entitled to do it, on that host, in `enforce` mode, with
/// its root defaulting to `$HOME`.
///
/// `CARGO_HOME` is honoured because a build host may move it off the boot
/// volume, which is exactly the kind of host that arms a disk janitor.
///
/// [`CACHEDIR_SIGNATURE`]: crate::deploy::host_build_caches::CACHEDIR_SIGNATURE
fn cargo_registry(home: &Path) -> PathBuf {
    match std::env::var_os("CARGO_HOME") {
        Some(value) if !value.is_empty() => PathBuf::from(value).join("registry"),
        _ => home.join(".cargo").join("registry"),
    }
}

pub(super) fn reserved_roots(home: &Path, policy: &DiskCleanupPolicy) -> Vec<PathBuf> {
    let mut roots = vec![
        home.join(STATE_DIR_PARTS[0]).join(STATE_DIR_PARTS[1]),
        home.join(".cache").join("huggingface").join("hub"),
        cargo_registry(home),
    ];
    // A CLI running out of a tagged build tree must not unlink its own
    // executable while it is restoring the host's ability to do work.
    if let Ok(executable) = std::env::current_exe() {
        roots.push(executable);
    }
    let configured_root = |name: &str, default: &[&str]| -> PathBuf {
        match policy.cleaners.get(name).and_then(|c| c.root.as_deref()) {
            Some(root) => crate::config_file::expand_tilde(root),
            None => default
                .iter()
                .fold(home.to_path_buf(), |path, part| path.join(part)),
        }
    };
    roots.push(configured_root(
        "weles_recordings",
        &["weles", "recordings"],
    ));
    if let Some(hf_root) = policy
        .cleaners
        .get("huggingface_cache")
        .and_then(|c| c.root.as_deref())
    {
        roots.push(crate::config_file::expand_tilde(hf_root));
    }
    roots
}

/// The macOS locations a build tool never writes a tagged cache into, and
/// which cost a privacy prompt or a network download to look inside.
///
/// `$HOME` is this cleaner's default root, so the walk reaches `~/Pictures`
/// like any other directory — and on 2026-09-09 it did, from the always-on
/// agent: `tccd` recorded `kTCCServicePhotos` requests attributed to
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
/// `Documents`, `Desktop` and `Downloads` are deliberately NOT here. They are
/// consent-gated too, but real build trees live in them — this fleet's own
/// checkouts are under `~/Documents` — so the honest cost is one grant
/// decision for a stably signed binary, not a permanent blind spot.
#[cfg(target_os = "macos")]
pub(super) fn privacy_protected_roots(home: &Path) -> Vec<PathBuf> {
    ["Pictures", "Music", "Movies", ".Trash"]
        .iter()
        .map(|part| home.join(part))
        .chain(
            ["Mobile Documents", "CloudStorage"]
                .iter()
                .map(|part| home.join("Library").join(part)),
        )
        .collect()
}

/// No operating system outside macOS gates these directories behind a consent
/// dialog, and a Linux build host may legitimately keep a tagged tree in any
/// of them.
#[cfg(not(target_os = "macos"))]
pub(super) fn privacy_protected_roots(_home: &Path) -> Vec<PathBuf> {
    Vec::new()
}
