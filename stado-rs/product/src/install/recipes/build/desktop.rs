//! A desktop product built from source in its checkout and placed as the
//! bundle the build produced. The checkout's `.build` is reused by the next
//! build, so between builds it is kept, tagged as a cache, in the checkout's
//! build area under Stado's home ([`build_tree`]) instead of being removed.

use super::{evidence, manifest};
use crate::{
    catalog::text,
    common::{checked, runs, Runtime},
    install::plan::{Placement, Prepared},
    install::settle::build_tree,
    source,
};
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::{
    ffi::OsString,
    fs,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
};

/// The variable naming the real `swift` to the `swift` a desktop build finds
/// first on its PATH, which is this Stado ([`swift_without_keychain`]).
const REAL_SWIFT: &str = "STADO_REAL_SWIFT";

/// When this process was started as `swift` (a desktop build's PATH puts this
/// Stado there), run the real SwiftPM with `--disable-keychain` and answer its
/// exit status; `None` for any other name. SwiftPM otherwise reads the login
/// keychain for every package host it fetches from, and a background build,
/// with nobody to answer macOS's access question, waited on that read for
/// hours inside `SecKeychainItemCopyContent`, holding the install and the
/// service that ran it. Without the keychain, git fetches through its own
/// credential helper as it does for any other build.
pub fn swift_without_keychain() -> Option<i32> {
    let mut arguments = std::env::args_os();
    let invoked = PathBuf::from(arguments.next()?);
    if invoked.file_name()? != "swift" {
        return None;
    }
    let Some(real) = std::env::var_os(REAL_SWIFT) else {
        eprintln!(
            "stado: started as swift without {REAL_SWIFT}, which only a desktop build Stado runs \
             sets; run the real swift instead"
        );
        return Some(127);
    };
    let mut forwarded: Vec<OsString> = arguments.collect();
    let fetches = matches!(
        forwarded.first().and_then(|word| word.to_str()),
        Some("build" | "test" | "run" | "package")
    );
    if fetches && !forwarded.iter().any(|word| word == "--disable-keychain") {
        forwarded.insert(1, OsString::from("--disable-keychain"));
    }
    let error = Command::new(&real).args(&forwarded).exec();
    eprintln!("stado: the real swift {} could not start: {error}", Path::new(&real).display());
    Some(127)
}

/// The directory a desktop build puts first on its PATH, whose `swift` is
/// this Stado, and the real swift it forwards to.
fn keychainless_swift(area: &Path) -> Result<(PathBuf, OsString)> {
    let found = checked(Command::new("/usr/bin/xcrun").args(["--find", "swift"]))
        .context("finding the swift a desktop build runs")?;
    let real = OsString::from(String::from_utf8(found.stdout)?.trim());
    let directory = area.join("swift-without-keychain");
    fs::create_dir_all(&directory).with_context(|| format!("creating {}", directory.display()))?;
    let link = directory.join("swift");
    if link.symlink_metadata().is_ok() {
        fs::remove_file(&link).with_context(|| format!("replacing {}", link.display()))?;
    }
    let stado = std::env::current_exe().context("naming this Stado for a desktop build's swift")?;
    std::os::unix::fs::symlink(&stado, &link)
        .with_context(|| format!("linking {} to {}", link.display(), stado.display()))?;
    Ok((directory, real))
}

pub fn desktop(
    runtime: &Runtime,
    product: &Value,
    recipe: &Value,
    root: &Path,
) -> Result<Prepared> {
    if !cfg!(target_os = "macos") {
        bail!("desktop bundle installation requires macOS");
    }
    let area = runs::checkout_area(&runtime.home, root)?;
    let index = area.join("source-index");
    let kept = area.join(build_tree::KEPT_TREE);
    let run = evidence::directory(&runtime.home, root)?;
    let evidence = run.path.clone();
    let recorded = source::snapshot(root, &evidence, &index)?;
    let document = if recipe["kind"] == "desktop-release" {
        manifest::load(root, text(recipe, "manifest")?)?
    } else {
        recipe.clone()
    };
    let command = text(
        &document,
        if recipe["kind"] == "desktop-release" {
            "build_command"
        } else {
            "command"
        },
    )?;
    build_tree::bring_back(root, &kept)?;
    let (swift_directory, real_swift) = keychainless_swift(&area)?;
    let path = std::env::var_os("PATH")
        .context("a desktop build searches this process's PATH for its tools, and it has none")?;
    let mut search = vec![swift_directory];
    search.extend(std::env::split_paths(&path));
    let built = checked(
        Command::new("/bin/sh")
            .args(["-c", command])
            .current_dir(root)
            .env("PATH", std::env::join_paths(search)?)
            .env(REAL_SWIFT, &real_swift)
            // A build script that runs Stado itself (`stado product swift
            // build`) reads the checkout it builds in this workspace; without
            // it, it read the default workspace and refused the checkout the
            // installation was building as not canonical.
            .env("WISENT_WORKSPACE", &runtime.workspace)
            .envs(manifest::secrets(&[&document])?),
    );
    if let Err(error) = built {
        return Err(match build_tree::put_away(root, &kept) {
            Ok(()) => error,
            Err(left) => error.context(format!(
                "the build tree also stayed in the checkout: {left:#}"
            )),
        });
    }
    source::verify_unchanged(root, &recorded, &evidence, &index)?;
    let source = if let Some(path) = document["bundle_path"].as_str() {
        manifest::inside(root, path)?
    } else {
        let directory = root.join(".build");
        let expected = directory.join(format!("{}.app", text(product, "name")?.replace(' ', "")));
        if expected.is_dir() {
            expected
        } else {
            let mut bundles = Vec::new();
            for entry in fs::read_dir(&directory)? {
                let path = entry?.path();
                if path.is_dir() && path.extension().is_some_and(|s| s == "app") {
                    bundles.push(path);
                }
            }
            let count = bundles.len();
            let mut found = bundles.into_iter();
            match (found.next(), found.next()) {
                (Some(only), None) => only,
                _ => bail!(
                    "desktop build produced {count} candidate bundles in {}",
                    directory.display()
                ),
            }
        }
    };
    if !source.is_dir() {
        bail!(
            "desktop build did not produce a bundle: {}",
            source.display()
        );
    }
    let destination = runtime
        .home
        .join("Applications")
        .join(source.file_name().context("bundle has no filename")?);
    Ok(Prepared {
        placements: vec![Placement {
            source,
            destination,
            symbolic: false,
        }],
        source_revision: recorded["revision"]
            .as_str()
            .context("snapshot has no revision")?
            .to_owned(),
        source_directory: Some(root.to_path_buf()),
        release: None,
        scratch: None,
        cache: Some(kept),
        resumed_from: None,
    })
}
