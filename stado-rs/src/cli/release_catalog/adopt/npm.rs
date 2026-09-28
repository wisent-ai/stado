//! What an npm package's own `package.json` says about what it publishes: the
//! name the release reads (its scope dropped), the version, the paths it ships
//! (`files`), and its test script. The release is the package's source
//! bundle, as echo's is: an npm package is not compiled, so the build packs
//! the shipped paths reproducibly and the post-build test is the package's own
//! `npm test`. A checkout without a readable `package.json`, a version, a
//! `files` list or a test script is refused rather than guessed: a release
//! with no post-build test waits at awaiting_tests forever.

use std::path::Path;

use serde_json::{json, Value};

use super::Planned;
use crate::cli::CmdError;
use crate::release_pipeline::PRODUCT_MANIFEST;

/// The platform a source bundle is built on; the bundle is the same bytes for
/// every host that installs it.
const PLATFORM: &str = "source-package";
const RUNNER: &str = "linux-amd64";
const PACKAGE: &str = "package.json";
/// Lock files npm, pnpm and yarn write beside the package; the one present is
/// bundled so an install reproduces the tested tree.
const LOCKS: [&str; 3] = ["package-lock.json", "pnpm-lock.yaml", "yarn.lock"];

/// The manifest an npm package is released with.
pub(super) fn files(checkout: &Path, product: &str) -> Result<Vec<Planned>, CmdError> {
    let path = checkout.join(PACKAGE);
    let text = std::fs::read_to_string(&path).map_err(|error| {
        CmdError::click(format!(
            "{} has no readable {PACKAGE}; --kind npm reads the package from it: {error}",
            checkout.display()
        ))
    })?;
    let package: Value = serde_json::from_str(&text)
        .map_err(|error| CmdError::click(format!("{} is not JSON: {error}", path.display())))?;
    let name = package["name"].as_str().unwrap_or_default();
    let unscoped = name.rsplit('/').next().unwrap_or(name);
    if unscoped != product {
        return Err(CmdError::click(format!(
            "{PACKAGE} names the package {name:?}, but the product is {product}; pass --product {unscoped}"
        )));
    }
    let version = package["version"].as_str().ok_or_else(|| {
        CmdError::click(format!("{} declares no version", path.display()))
    })?;
    let shipped: Vec<&str> = package["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    if shipped.is_empty() {
        return Err(CmdError::click(format!(
            "{} declares no files list; the release bundles exactly what the package publishes",
            path.display()
        )));
    }
    if let Some(missing) = shipped.iter().find(|entry| !checkout.join(entry).exists()) {
        return Err(CmdError::click(format!(
            "{} lists {missing:?} in files, which the checkout does not hold",
            path.display()
        )));
    }
    let bundle = format!("{product}-source.tar");
    let mut build = vec!["stado", "product", "source-bundle", "--name", bundle.as_str()];
    let locks = LOCKS.iter().filter(|lock| checkout.join(lock).is_file());
    for include in [PACKAGE].iter().chain(locks).chain(shipped.iter()) {
        build.extend(["--include", *include]);
    }
    if !matches!(package["scripts"]["test"].as_str(), Some(script) if !script.trim().is_empty()) {
        return Err(CmdError::click(format!(
            "{} declares no scripts.test; a release qualifies only on its post-build test, \
             so add the package's test script before adopting it",
            path.display()
        )));
    }
    let tests = json!([{"name": "npm-test", "argv": ["npm", "test"]}]);
    let manifest = json!({
        "schema_version": 1,
        "product": product,
        "releases": true,
        "version_source": {"kind": "json", "path": PACKAGE, "pointer": "/version"},
        "platforms": {
            PLATFORM: {
                "runner_platform": RUNNER,
                "quality": [],
                "build": {"argv": build},
                "tests": tests,
                "stage": {
                    format!("release/{bundle}"): bundle,
                    "release/SOURCE_REVISION": "SOURCE_REVISION"
                },
                "secret_env": {}
            }
        },
        "promotion": {"channels": ["candidate"], "reconcile": false}
    });
    let text = serde_json::to_string_pretty(&manifest)
        .map_err(|error| CmdError::click(error.to_string()))?
        + "\n";
    eprintln!("{product}: {PACKAGE} version {version}, ships {}", shipped.join(" "));
    Ok(vec![Planned {
        path: checkout.join(PRODUCT_MANIFEST),
        text,
        executable: false,
    }])
}
