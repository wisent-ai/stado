//! Placing bytes on a host: one resolved artifact reference, or one release
//! archive that is not in an object store yet.

use super::*;

pub(super) mod archive;
pub(super) mod current;

/// Resolve one artifact reference and place that exact version on the host.
///
/// The alias is resolved before anything is written, so what lands on disk is
/// an immutable version and the path names it. Verification happens on the
/// host against the digest the manifest declares: a download that does not
/// match never becomes a running unit, and the previous `current` is left
/// where it was.
pub(crate) async fn install_from_artifact(
    target: &crate::targets::ComputeTarget,
    name: &str,
    reference: &str,
) -> Result<crate::deploy::artifact_install::InstalledArtifact, CmdError> {
    let registry = crate::artifacts::ArtifactRegistry::new()
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))?;
    let parsed = crate::artifacts_models::ArtifactRef::parse(reference)?;
    let manifest = registry.resolve_manifest(&parsed).await?;
    let runner = production_runner();
    crate::deploy::artifact_install::install_artifact(target, name, &manifest, &runner)
        .await
        .map_err(click)
}

/// Install a release archive that is not in an object store yet.
///
/// The published route is `--from-artifact`, and it stays the durable one. This
/// exists because a bundle has to reach a host before the fleet has a store
/// both machines can read, and the alternative people reach for in that gap is
/// copying a file by hand onto a running service. The archive is streamed over
/// the approved channel, checksummed on the far side, unpacked into a version
/// directory named for its own digest, and `current` is relinked only after the
/// digest matches.
/// The paths a gzip-compressed release archive carries, in archive order.
///
/// Read locally, before anything is copied to a host: the cheapest moment to
/// learn that a bundle cannot satisfy the unit it is meant for.
pub(super) fn archive_members(path: &str) -> Result<Vec<String>, CmdError> {
    let file = std::fs::File::open(path)
        .map_err(|error| CmdError::click(format!("cannot read archive {path}: {error}")))?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let entries = archive
        .entries()
        .map_err(|error| CmdError::click(format!("{path} is not a tar archive: {error}")))?;
    let mut members = Vec::new();
    for entry in entries {
        let entry = entry
            .map_err(|error| CmdError::click(format!("{path} could not be listed: {error}")))?;
        let path = entry.path().map_err(|error| {
            CmdError::click(format!("{path} holds an unreadable name: {error}"))
        })?;
        members.push(normalize_member(&path.to_string_lossy()));
    }
    Ok(members)
}

/// One archive path, comparable: no `./` prefix, no trailing slash.
fn normalize_member(raw: &str) -> String {
    raw.trim_start_matches("./")
        .trim_end_matches('/')
        .to_string()
}

/// Refuse an archive that does not carry the file the unit executes after the
/// installer adds its fixed `darwin-arm/` platform directory.
///
/// The unit's program is an absolute path through `current`, while
/// [`ARCHIVE_INSTALL_BODY`] extracts the archive inside
/// `version_dir/darwin-arm`. A unit running
/// `.../current/darwin-arm/stado` therefore needs a root archive member
/// `stado`, not a second `darwin-arm/stado` nesting. Both sides are named in
/// the refusal, because the useful sentence is the mismatch, not the fact of
/// one.
pub(super) fn refuse_archive_without_program(
    program: &str,
    members: &[String],
) -> Result<(), String> {
    let Some(relative) = program.split("/current/").nth(usize::from(true)) else {
        // A unit pinned to a version directory rather than `current` is a
        // different fault, reported by `follow_current`; there is no program
        // path to look for here and inventing one would refuse every archive.
        return Ok(());
    };
    let relative = normalize_member(relative);
    let Some(archive_relative) = relative.strip_prefix("darwin-arm/") else {
        return Err(format!(
            "refusing to relink `current`: the archive installer writes below \
             current/darwin-arm, but the unit runs current/{relative}"
        ));
    };
    if archive_relative.is_empty() || members.iter().any(|member| member == archive_relative) {
        return Ok(());
    }
    let mut held: Vec<&str> = members
        .iter()
        .filter(|member| !member.ends_with('/'))
        .map(String::as_str)
        .take(6)
        .collect();
    if members.len() > held.len() {
        held.push("…");
    }
    Err(format!(
        "refusing to relink `current`: the unit runs current/{relative}; the archive holds {}. \
         Pointing `current` at a tree without that file does not fail here - it fails at \
         launchd's next spawn, which cannot report why, and a KeepAlive job that cannot spawn \
         leaves its domain. Install an archive whose layout matches the unit's program, or \
         change the unit's program to a path this archive carries.",
        if held.is_empty() {
            "nothing".to_string()
        } else {
            held.join(", ")
        }
    ))
}
