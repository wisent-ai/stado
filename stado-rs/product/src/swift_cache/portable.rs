//! Making a SwiftPM dependency cache portable before it is published: every
//! artifact path and every checkout's borrowed object store relative to the
//! cache, never to the producer's scratch.

use super::{read, STATE};
use crate::common::{atomic_json, relative};
use anyhow::{bail, Context, Result};
use serde_json::json;
use std::{fs, path::Path};

/// SwiftPM clones each checkout from its mirror under `repositories/` and the
/// clone borrows the mirror's objects through `.git/objects/info/alternates`,
/// which names the mirror by the producer's absolute path. Unpacked anywhere
/// else, every checkout pointed at a stage that no longer existed and Git
/// refused it (`unable to normalize alternate object path: …/release-input/
/// swiftpm-…/.build/repositories/echo-…/objects`). Git reads a relative
/// alternate from the objects directory, so each is rewritten relative to its
/// own checkout, and the layout the archive keeps keeps it valid.
fn relative_alternates(scratch: &Path) -> Result<()> {
    let checkouts = scratch.join("checkouts");
    if !checkouts.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(&checkouts)? {
        let objects = entry?.path().join(".git").join("objects");
        let alternates = objects.join("info").join("alternates");
        if !alternates.is_file() {
            continue;
        }
        let depth = objects.strip_prefix(scratch)?.components().count();
        let up: std::path::PathBuf = std::iter::repeat_n("..", depth).collect();
        let mut rewritten = String::new();
        for line in fs::read_to_string(&alternates)?.lines() {
            let target = Path::new(line).canonicalize().with_context(|| {
                format!(
                    "{} names {line}, which is unavailable",
                    alternates.display()
                )
            })?;
            let inside = target.strip_prefix(scratch).with_context(|| {
                format!(
                    "{} borrows objects from {}, outside {}",
                    alternates.display(),
                    target.display(),
                    scratch.display()
                )
            })?;
            rewritten.push_str(&up.join(inside).to_string_lossy());
            rewritten.push('\n');
        }
        // SwiftPM leaves a checkout read-only; the file is opened for this
        // one write and given back its read-only mode.
        let mut permissions = fs::metadata(&alternates)?.permissions();
        let read_only = permissions.readonly();
        // Writable for every class only for this write; the mode it had is
        // set back right after.
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(&alternates, permissions.clone())?;
        fs::write(&alternates, rewritten)
            .with_context(|| format!("writing {}", alternates.display()))?;
        permissions.set_readonly(read_only);
        fs::set_permissions(&alternates, permissions)?;
    }
    Ok(())
}

/// Called before publication, while the producer's actual files still exist.
pub fn make_portable(scratch: &Path) -> Result<()> {
    let scratch = scratch.canonicalize()?;
    relative_alternates(&scratch)?;
    let state_path = scratch.join(STATE);
    let mut state = read(&state_path)?;
    for artifact in state["object"]["artifacts"]
        .as_array_mut()
        .context("Swift workspace state has no artifacts array")?
    {
        let path = Path::new(
            artifact["path"]
                .as_str()
                .context("Swift artifact has no path")?,
        );
        let portable = path.strip_prefix(&scratch).with_context(|| {
            format!(
                "Swift artifact {} is outside {}",
                path.display(),
                scratch.display()
            )
        })?;
        relative(portable)?;
        if !path.canonicalize()?.starts_with(&scratch) {
            bail!("Swift artifact escapes its cache: {}", path.display());
        }
        artifact["path"] = json!(portable);
    }
    atomic_json(&state_path, &state)
}
