//! What an npm package's own `package.json` says about what it publishes: the
//! name the release reads (its scope dropped), the version and the paths it
//! ships (`files`). The release is the package's source bundle, as echo's is:
//! an npm package is not compiled, so the build packs the shipped paths
//! reproducibly. A checkout without a readable `package.json`, a version or a
//! `files` list is refused rather than guessed. The quality gate is
//! `release/fmt.sh --check` of kind `format`, Biome over the shipped paths,
//! because `stado release changes submit` hands off only a commit whose
//! product declares a `format` gate. The manifest declares no post-build test: a test is added
//! only once the operator approves it.

use std::path::Path;

use serde_json::{json, Value};

use super::{fill, Planned, BIOME_FMT};
use crate::cli::CmdError;
use crate::primitives::presence::present;
use crate::release_pipeline::{GateKind, PRODUCT_MANIFEST, SCHEMA_VERSION};

/// The platform a source bundle is built on; the bundle is the same bytes for
/// every host that installs it.
const PLATFORM: &str = "source-package";
const RUNNER: &str = "linux-amd64";
const PACKAGE: &str = "package.json";
/// Lock files npm, pnpm and yarn write beside the package; the one present is
/// bundled so an install reproduces the tested tree.
const LOCKS: &[&str] = &["package-lock.json", "pnpm-lock.yaml", "yarn.lock"];

/// The manifest an npm package is released with.
pub(super) fn files(checkout: &Path, product: &str) -> Result<Vec<Planned>, CmdError> {
    let path = checkout.join(PACKAGE);
    let text = std::fs::read_to_string(&path).map_err(|error| {
        CmdError::refused(format!(
            "{} has no readable {PACKAGE}; --kind npm reads the package from it: {error}",
            checkout.display()
        ))
    })?;
    let package: Value = serde_json::from_str(&text)
        .map_err(|error| CmdError::refused(format!("{} is not JSON: {error}", path.display())))?;
    // A package without a name is refused as that; read as "" it was refused
    // as a name mismatch that told the operator to pass an empty --product.
    let name = package["name"].as_str().ok_or_else(|| {
        CmdError::refused(format!(
            "{} declares no package name; --kind npm releases the package under its name",
            path.display()
        ))
    })?;
    // A scoped name (`@scope/name`) is released under its last segment.
    let unscoped = match name.rsplit_once('/') {
        Some((_, last)) => last,
        None => name,
    };
    if unscoped != product {
        return Err(CmdError::refused(format!(
            "{PACKAGE} names the package {name:?}, but the product is {product}; pass --product {unscoped}"
        )));
    }
    let version = package["version"]
        .as_str()
        .ok_or_else(|| CmdError::refused(format!("{} declares no version", path.display())))?;
    let shipped: Vec<&str> = package["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if shipped.is_empty() {
        return Err(CmdError::refused(format!(
            "{} declares no files list; the release bundles exactly what the package publishes",
            path.display()
        )));
    }
    for entry in &shipped {
        if !present(&checkout.join(entry))? {
            return Err(CmdError::refused(format!(
                "{} lists {entry:?} in files, which the checkout does not hold",
                path.display()
            )));
        }
    }
    let bundle = format!("{product}-source.tar");
    let mut build = vec![
        "stado",
        "product",
        "source-bundle",
        "--name",
        bundle.as_str(),
    ];
    // Every lock file the checkout holds is bundled; one that cannot be
    // examined is refused with its cause rather than left out of the bundle.
    let mut locks = Vec::new();
    for lock in LOCKS {
        if present(&checkout.join(lock))? {
            locks.push(lock);
        }
    }
    for include in [PACKAGE].iter().chain(locks).chain(shipped.iter()) {
        build.extend(["--include", *include]);
    }
    let manifest = json!({
        "schema_version": SCHEMA_VERSION,
        "product": product,
        "releases": true,
        "version_source": {"kind": "json", "path": PACKAGE, "pointer": "/version"},
        "platforms": {
            PLATFORM: {
                "runner_platform": RUNNER,
                "quality": [{
                    "name": "fmt",
                    "argv": ["bash", "release/fmt.sh", "--check"],
                    "kind": GateKind::Format,
                }],
                "build": {"argv": build},
                "stage": {
                    format!("release/{bundle}"): bundle,
                    "release/SOURCE_REVISION": "SOURCE_REVISION"
                },
                "secret_env": {}
            }
        },
        "promotion": {"channels": ["candidate"], "reconcile": false}
    });
    let text = serde_json::to_string_pretty(&manifest).map_err(CmdError::from)? + "\n";
    eprintln!(
        "{product}: {PACKAGE} version {version}, ships {}",
        shipped.join(" ")
    );
    let sources = shipped.join(" ");
    let values = [("PRODUCT", product), ("SOURCES", sources.as_str())];
    Ok(vec![
        Planned {
            path: checkout.join(PRODUCT_MANIFEST),
            text,
            executable: false,
        },
        Planned {
            path: checkout.join("release/fmt.sh"),
            text: fill(BIOME_FMT, &values),
            executable: true,
        },
    ])
}
