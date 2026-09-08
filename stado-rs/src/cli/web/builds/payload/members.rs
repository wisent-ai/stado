//! Every path that goes into the artifact, in the one order two builds of one
//! commit will always produce -- and everything that never goes in.

use std::path::{Path, PathBuf};

use crate::cli::CmdError;

/// Build outputs of the Next.js compiler that must never enter the artifact.
///
/// `.next/cache` is the webpack and SWC cache: cache packs embed absolute
/// paths from the builder's checkout and their own write times, so two builds
/// of one commit differ inside it. `.next/trace` is the build's own telemetry
/// trace, which is nothing but timestamps. Neither is read at runtime — Next
/// documents `.next/cache` as build-only state — so excluding them costs the
/// unit nothing and is what makes the tarball reproducible at all.
const EXCLUDED_BUILD_OUTPUT: [&str; 2] = ["cache", "trace"];

/// What never enters a static artifact, when the site root is the checkout
/// root itself.
///
/// Four of these sites are the repository: `index.html` and a stylesheet at
/// the top level, which is exactly what Vercel served. Staging that directory
/// wholesale would put the git history, the developer's `.env.local` and the
/// Vercel project link inside a published artifact — and then serve them, at
/// `/.env.local`, to anyone who asked. None of them is part of the site, and
/// one of them is a credential.
///
/// `.wisent-release.json` is on the list for the same reason: it names the
/// Skarbiec items and fields this product's build reads, which is a map of
/// the vault nobody needs to publish.
const NOT_PART_OF_A_SITE: [&str; 10] = [
    ".git",
    ".gitignore",
    ".github",
    ".vercel",
    ".vercelignore",
    "vercel.json",
    ".next",
    "node_modules",
    "release",
    crate::release_pipeline::PRODUCT_MANIFEST,
];

/// Every path that goes into a static site's artifact, name-sorted.
///
/// The exclusions apply to whichever directory the site root is, not only to
/// the checkout root. `jeden`'s site is its `web` directory and carries a
/// `.vercelignore` and a `vercel.json`; a `dist/` written by a build can pick
/// up a `.env.local` the same way. Everything on this list is developer or
/// platform state rather than part of the site, one entry is a credential,
/// and a static server publishes whatever is in the directory — so `/.env.local`
/// would be a request anyone could make.
pub(super) fn static_members(root: &Path) -> Result<Vec<PathBuf>, CmdError> {
    // A site with nothing in it installs as a unit that answers 404 to
    // everything, and reports a successful deploy while doing it.
    if !root.join("index.html").is_file() {
        return Err(CmdError::click(format!(
            "{} has no index.html after the build: that is the file the static server answers `/` with, so there is no site to stage",
            root.display()
        )));
    }
    let excluded: Vec<PathBuf> = NOT_PART_OF_A_SITE
        .iter()
        .map(|name| root.join(name))
        .collect();
    let mut members = Vec::new();
    walk(root, &excluded, &mut members)?;
    // A dotfile carrying environment values is developer state wherever it
    // sits, and the name is the only thing that identifies it.
    members.retain(|member| {
        !member
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(".env"))
    });
    Ok(members)
}

/// Every path that goes into a served product's artifact, in the one order
/// two builds of a commit will always produce: a fixed root order, then each
/// directory's entries sorted by name.
pub(super) fn members(source: &Path) -> Result<Vec<PathBuf>, CmdError> {
    // Required members, each refused with what its absence means. A tarball
    // missing any of them installs as a unit that cannot start.
    let required: [(&str, &str); 4] = [
        (
            "package.json",
            "the unit runs the product's own `start` script through it",
        ),
        (
            "package-lock.json",
            "the artifact records the tree it was installed from",
        ),
        (
            ".next",
            "`npm run build` produced no build output, so the product's build script did not build a Next.js application",
        ),
        (
            "node_modules",
            "the unit runs from the artifact and installs nothing at deploy time",
        ),
    ];
    for (name, why) in required {
        if !source.join(name).exists() {
            return Err(CmdError::click(format!(
                "{} is missing after the build: {why}",
                source.join(name).display()
            )));
        }
    }

    let mut roots = vec![
        source.join("package.json"),
        source.join("package-lock.json"),
        source.join(".next"),
    ];
    if source.join("public").is_dir() {
        roots.push(source.join("public"));
    }
    // `next.config.ts`, `.js`, `.mjs` — the extension is the product's choice,
    // and the runtime reads whichever one it finds, so all of them travel.
    let mut entries: Vec<PathBuf> = std::fs::read_dir(source)
        .map_err(|error| CmdError::click(format!("cannot read {}: {error}", source.display())))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("next.config."))
        })
        .collect();
    entries.sort();
    roots.extend(entries);
    roots.push(source.join("node_modules"));

    let excluded: Vec<PathBuf> = EXCLUDED_BUILD_OUTPUT
        .iter()
        .map(|name| source.join(".next").join(name))
        .collect();
    let mut members = Vec::new();
    for root in roots {
        members.push(root.clone());
        if std::fs::symlink_metadata(&root)?.is_dir() {
            walk(&root, &excluded, &mut members)?;
        }
    }
    Ok(members)
}

/// One directory's contents, name-sorted, then each subdirectory's.
///
/// The recursion tests the entry with `symlink_metadata`, so a link to a
/// directory is recorded and not descended into. `node_modules` can hold a
/// link back into itself, and descending one is how a walk of it never ends.
fn walk(directory: &Path, excluded: &[PathBuf], into: &mut Vec<PathBuf>) -> Result<(), CmdError> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(directory)
        .map_err(|error| CmdError::click(format!("cannot read {}: {error}", directory.display())))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()?;
    entries.sort();
    for entry in entries {
        if excluded.contains(&entry) {
            continue;
        }
        into.push(entry.clone());
        if std::fs::symlink_metadata(&entry)?.is_dir() {
            walk(&entry, excluded, into)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_site_root_never_stages_git_platform_or_developer_state() {
        // Whatever directory the site root is. Four of these sites are the
        // repository itself; `jeden`'s is its `web` directory and carries a
        // .vercelignore and a vercel.json. A static server publishes whatever
        // is in the directory, so `/.env.local` would be a request anyone
        // could make.
        for name in [
            ".git",
            ".gitignore",
            ".vercel",
            ".vercelignore",
            "vercel.json",
            "node_modules",
            ".wisent-release.json",
        ] {
            assert!(
                NOT_PART_OF_A_SITE.contains(&name),
                "{name} must never be staged"
            );
        }
    }
}
