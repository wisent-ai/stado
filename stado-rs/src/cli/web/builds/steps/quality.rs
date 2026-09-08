//! `stado web quality` -- the gate that runs before the build.

use crate::cli::web::builds::contract::package::kind::Kind;
use crate::cli::web::builds::contract::package::version::{
    require_version, version_source_is_package_json,
};
use crate::cli::web::builds::contract::package::{manifest_if_present, script};
use crate::cli::web::builds::contract::release::env::declared_env;
use crate::cli::web::builds::contract::release::product;
use crate::cli::web::builds::contract::release::site::site_root;
use crate::cli::web::builds::contract::worker::worker;
use crate::cli::web::builds::tooling::install::install;
use crate::cli::web::builds::tooling::node::npm;
use crate::cli::web::LAUNCHER;
use crate::cli::CmdError;

pub(crate) fn quality(declared_root: Option<&str>) -> Result<(), CmdError> {
    let worker = worker()?;
    worker.require_web_platform()?;
    let manifest = manifest_if_present(&worker.source)?;
    // A version is checked against `package.json` only where there is one to
    // check against. A static site's version comes from whatever its
    // `version_source` names, and the pipeline has already read it — this
    // gate exists to catch a builder building the wrong checkout, and for a
    // product with no package manifest there is no second copy to disagree.
    if let Some(manifest) = &manifest {
        if version_source_is_package_json(&worker.source) {
            require_version(manifest, &worker.version)?;
        }
    }
    let product = product(&worker.source, manifest.as_ref())?;
    let kind = Kind::of(manifest.as_ref());
    let variables = declared_env(&worker.source, &worker.platform)?;
    println!(
        "stado web quality: {product} {} in {} ({})",
        worker.version,
        worker.source.display(),
        worker.inputs_report()
    );

    let builds = manifest
        .as_ref()
        .and_then(|manifest| script(manifest, "build"))
        .is_some();
    match kind {
        Kind::Server => {
            // `build` is what produces `.next`; `start` is what the generated
            // launcher executes on the unit's port. This product declared a
            // `start`, so it is a server, and a server with nothing to build
            // has no build output for the artifact to carry. Checked before
            // anything is installed, because an install of a tree Stado could
            // never run afterwards wastes the whole gate.
            if !builds {
                return Err(CmdError::click(format!(
                    "package.json declares a `start` script but no `build` script: a served web product needs both — `build`, which produces .next, and `start`, which the generated {} launcher runs on the unit's port",
                    LAUNCHER
                )));
            }
        }
        // A site whose build writes its own root has no root yet: `dist/` does
        // not exist until the build runs, and resolving it here would fail
        // every built static site before its gate had done anything. That
        // check belongs after the build, in `static_members`, which is where
        // the artifact is assembled from it.
        Kind::Static if builds => println!(
            "stado web quality: {product} is a static site its build writes; the site root is resolved after the build"
        ),
        Kind::Static => {
            // The files are committed, so the one thing this product must
            // have is checkable right now.
            let root = site_root(&worker.source, declared_root)?;
            if !root.join("index.html").is_file() {
                return Err(CmdError::click(format!(
                    "{} has no index.html and the product declares no build script: a static web product is a directory of files, and `--root` names which directory",
                    root.display()
                )));
            }
            println!(
                "stado web quality: {product} is a static site at {} (no `start` script, so nothing is started; the {} launcher serves the directory)",
                root.display(),
                LAUNCHER
            );
        }
    }

    // A product with no package.json has no locked tree to install and no
    // scripts to run. Refusing it here would be Stado requiring a Node
    // package of a product that is four files and a stylesheet.
    let Some(manifest) = manifest else {
        println!(
            "stado web quality: {product} carries no package.json; the gate is that the site root exists and holds an index.html"
        );
        return Ok(());
    };

    // A static site with no build script has a package.json that the release
    // does not use: nothing is built from the tree and no node_modules is
    // staged, so installing it gates the release on dependencies the artifact
    // never carries. `jeden` is a Rust CLI whose repository root has a
    // `package.json` for its npm wrapper, with a `file:` dependency on a
    // vendored tarball; `npm ci` there exits 254 and would fail the release of
    // a site made of committed HTML.
    if kind == Kind::Static && !builds {
        println!(
            "stado web quality: {product} declares no build script, so its package.json is not part of this release and nothing is installed; the gate is that the site root exists and holds an index.html"
        );
        return Ok(());
    }

    install(&worker.source, &variables)?;

    // The product's own checks, not Stado's opinion of them. A landing site
    // with neither script is a legitimate web product; it just has nothing
    // here to run, and the log says so rather than leaving the operator to
    // wonder which check passed.
    let mut ran = Vec::new();
    for check in ["typecheck", "lint"] {
        if script(&manifest, check).is_some() {
            npm(&worker.source, &["run", check], &variables)?;
            ran.push(check);
        }
    }
    if ran.is_empty() {
        println!(
            "stado web quality: {product} declares neither a typecheck nor a lint script; the locked install is the whole gate"
        );
    } else {
        println!("stado web quality: {product} passed {}", ran.join(" and "));
    }
    Ok(())
}
