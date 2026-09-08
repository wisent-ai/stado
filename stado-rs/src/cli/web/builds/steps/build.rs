//! `stado web build` -- the step that stages the runnable artifact.

use crate::cli::web::builds::contract::naming::{sidecar_line, tarball_name, top_level};
use crate::cli::web::builds::contract::package::kind::Kind;
use crate::cli::web::builds::contract::package::version::{
    require_version, version_source_is_package_json,
};
use crate::cli::web::builds::contract::package::{manifest_if_present, script};
use crate::cli::web::builds::contract::release::env::declared_env;
use crate::cli::web::builds::contract::release::product;
use crate::cli::web::builds::contract::release::site::site_root;
use crate::cli::web::builds::contract::worker::worker;
use crate::cli::web::builds::payload::archive::{digest, stage};
use crate::cli::web::builds::tooling::install::install;
use crate::cli::web::builds::tooling::node::npm;
use crate::cli::web::builds::tooling::revision::revision;
use crate::cli::CmdError;

pub(crate) fn build(declared_root: Option<&str>) -> Result<(), CmdError> {
    let worker = worker()?;
    worker.require_web_platform()?;
    let manifest = manifest_if_present(&worker.source)?;
    if let Some(manifest) = &manifest {
        if version_source_is_package_json(&worker.source) {
            require_version(manifest, &worker.version)?;
        }
    }
    let product = product(&worker.source, manifest.as_ref())?;
    let kind = Kind::of(manifest.as_ref());
    let variables = declared_env(&worker.source, &worker.platform)?;
    println!(
        "stado web build: {product} {} in {} ({})",
        worker.version,
        worker.source.display(),
        worker.inputs_report()
    );

    // A build runs when the product declares one. A static site whose files
    // are committed declares none, and running `npm run build` on it would
    // fail on a script that does not exist.
    if manifest
        .as_ref()
        .and_then(|manifest| script(manifest, "build"))
        .is_some()
    {
        // The worker may run the build in a checkout that never saw the
        // quality step — a re-run of one platform, or a recipe with no
        // quality gate — so the install is repeated when, and only when,
        // there is no tree to build against.
        if worker.source.join("node_modules").is_dir() {
            println!("stado web build: node_modules is present from the quality step");
        } else {
            install(&worker.source, &variables)?;
        }
        npm(&worker.source, &["run", "build"], &variables)?;
    } else {
        println!(
            "stado web build: {product} declares no build script; its committed files are the site"
        );
    }

    // Resolved after the build, because a site root a build script writes
    // does not exist before it runs.
    let root = site_root(&worker.source, declared_root)?;
    let revision = revision(&worker.source)?;
    let dist = worker.output.join("dist");
    std::fs::create_dir_all(&dist)
        .map_err(|error| CmdError::click(format!("cannot create {}: {error}", dist.display())))?;
    let file_name = tarball_name(&product);
    let tarball = dist.join(&file_name);
    let top = top_level(&product, &worker.version);
    stage(&worker.source, &root, kind, &tarball, &top)?;

    // The digest is streamed rather than taken over the whole file in memory:
    // a tarball carrying node_modules runs to hundreds of megabytes, and the
    // builder is a fleet host with other work on it.
    let digest = digest(&tarball)?;
    let sidecar = dist.join(format!("{file_name}.sha256"));
    std::fs::write(&sidecar, sidecar_line(&digest, &file_name))
        .map_err(|error| CmdError::click(format!("cannot write {}: {error}", sidecar.display())))?;
    let source_revision = dist.join("SOURCE_REVISION");
    std::fs::write(&source_revision, format!("{revision}\n")).map_err(|error| {
        CmdError::click(format!(
            "cannot write {}: {error}",
            source_revision.display()
        ))
    })?;

    let bytes = std::fs::metadata(&tarball)?.len();
    println!(
        "stado web build: staged {} ({bytes} bytes, sha256 {digest}) from {revision}",
        tarball.display()
    );
    Ok(())
}
