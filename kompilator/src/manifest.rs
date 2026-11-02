//! What one package's `Cargo.toml` says a build needs: its name, version,
//! edition, library target, build script, `links`, features and the
//! dependencies of each kind with the platforms they apply to. A field a
//! package inherits from its workspace (`x.workspace = true`) is read from
//! the workspace root above it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::toml::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Normal,
    Build,
}

#[derive(Clone, Debug)]
pub struct Dependency {
    /// The name the depending crate calls it by (the table key).
    pub alias: String,
    /// The package's own name (`package = "…"`, else the alias).
    pub package: String,
    pub requirement: Option<String>,
    pub path: Option<PathBuf>,
    pub optional: bool,
    pub default_features: bool,
    pub features: Vec<String>,
    pub kind: Kind,
    /// The `cfg(…)` expression or target triple of a `[target.…]` table.
    pub platform: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Manifest {
    pub dir: PathBuf,
    pub name: String,
    pub version: String,
    pub edition: String,
    pub links: Option<String>,
    pub lib: Option<Lib>,
    pub bins: Vec<Bin>,
    pub build_script: Option<PathBuf>,
    pub features: BTreeMap<String, Vec<String>>,
    pub dependencies: Vec<Dependency>,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct Lib {
    pub crate_name: String,
    pub path: PathBuf,
    pub proc_macro: bool,
    pub edition: String,
}

#[derive(Clone, Debug)]
pub struct Bin {
    pub name: String,
    pub path: PathBuf,
}

fn text(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(String::from)
}

/// The workspace root manifest above `dir`, when one is there.
fn workspace(dir: &Path) -> Option<Value> {
    let mut here = dir.parent();
    while let Some(candidate) = here {
        let manifest = candidate.join("Cargo.toml");
        if let Ok(read) = std::fs::read_to_string(&manifest) {
            if let Ok(document) = crate::toml::parse(&read) {
                if document.get("workspace").is_some() {
                    return Some(document);
                }
            }
        }
        here = candidate.parent();
    }
    None
}

/// A `[package]` field, or the workspace's when the package inherits it.
fn package_field(package: &Value, workspace: Option<&Value>, key: &str) -> Option<String> {
    match package.get(key) {
        Some(Value::String(value)) => Some(value.clone()),
        Some(table @ Value::Table(_)) if table.get("workspace").and_then(Value::as_bool) == Some(true) => {
            text(workspace?.get("workspace")?.get("package")?.get(key))
        }
        _ => None,
    }
}

fn dependency(
    alias: &str,
    spec: &Value,
    kind: Kind,
    platform: Option<&str>,
    dir: &Path,
    workspace: Option<&Value>,
) -> Result<Dependency, String> {
    let mut spec = spec.clone();
    let mut inherited_features = Vec::new();
    let mut optional = spec.get("optional").and_then(Value::as_bool) == Some(true);
    if spec.get("workspace").and_then(Value::as_bool) == Some(true) {
        let base = workspace
            .and_then(|root| root.get("workspace"))
            .and_then(|table| table.get("dependencies"))
            .and_then(|table| table.get(alias))
            .ok_or_else(|| format!("{alias} inherits its dependency from a workspace that does not declare it"))?;
        inherited_features = spec.get("features").map(Value::strings).unwrap_or_default();
        optional = spec.get("optional").and_then(Value::as_bool) == Some(true);
        spec = base.clone();
    }
    let (requirement, path, default_features, features, package) = match &spec {
        Value::String(requirement) => (Some(requirement.clone()), None, true, Vec::new(), alias.to_string()),
        Value::Table(_) => {
            let default_features = spec
                .get("default-features")
                .or_else(|| spec.get("default_features"))
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let path = text(spec.get("path")).map(|path| dir.join(path));
            (
                text(spec.get("version")),
                path,
                default_features,
                spec.get("features").map(Value::strings).unwrap_or_default(),
                text(spec.get("package")).unwrap_or_else(|| alias.to_string()),
            )
        }
        _ => return Err(format!("the dependency {alias} is neither a version nor a table")),
    };
    let mut features = features;
    features.extend(inherited_features);
    Ok(Dependency {
        alias: alias.to_string(),
        package,
        requirement,
        path,
        optional: optional || spec.get("optional").and_then(Value::as_bool) == Some(true),
        default_features,
        features,
        kind,
        platform: platform.map(String::from),
    })
}

fn dependencies_of(
    table: Option<&Value>,
    kind: Kind,
    platform: Option<&str>,
    dir: &Path,
    workspace: Option<&Value>,
    into: &mut Vec<Dependency>,
) -> Result<(), String> {
    for (alias, spec) in table.and_then(Value::as_table).into_iter().flatten() {
        into.push(dependency(alias, spec, kind, platform, dir, workspace)?);
    }
    Ok(())
}

impl Manifest {
    pub fn read(dir: &Path) -> Result<Manifest, String> {
        let path = dir.join("Cargo.toml");
        let read = std::fs::read_to_string(&path).map_err(|error| format!("{} cannot be read: {error}", path.display()))?;
        let document = crate::toml::parse(&read).map_err(|why| format!("{} is not readable TOML: {why}", path.display()))?;
        let package = document.get("package").ok_or_else(|| format!("{} has no [package]", path.display()))?;
        let root = workspace(dir);
        let name = package_field(package, root.as_ref(), "name").ok_or_else(|| format!("{} names no package", path.display()))?;
        let version = package_field(package, root.as_ref(), "version").unwrap_or_else(|| String::from("0.0.0"));
        let edition = package_field(package, root.as_ref(), "edition").unwrap_or_else(|| String::from("2015"));
        let links = text(package.get("links"));

        let lib_table = document.get("lib");
        let default_lib = dir.join("src/lib.rs");
        let lib_path = match text(lib_table.and_then(|lib| lib.get("path"))) {
            Some(path) => Some(dir.join(path)),
            None => default_lib.exists().then_some(default_lib),
        };
        let proc_macro = lib_table
            .and_then(|lib| lib.get("proc-macro").or_else(|| lib.get("proc_macro")))
            .and_then(Value::as_bool)
            == Some(true);
        let lib = lib_path.map(|path| Lib {
            crate_name: text(lib_table.and_then(|lib| lib.get("name"))).unwrap_or_else(|| name.replace('-', "_")),
            path,
            proc_macro,
            edition: text(lib_table.and_then(|lib| lib.get("edition"))).unwrap_or_else(|| edition.clone()),
        });

        let mut bins = Vec::new();
        for bin in document.get("bin").and_then(Value::as_array).into_iter().flatten() {
            let bin_name = text(bin.get("name")).ok_or_else(|| format!("{} has a [[bin]] without a name", path.display()))?;
            let bin_path = text(bin.get("path")).map(|path| dir.join(path)).unwrap_or_else(|| {
                let main = dir.join("src/main.rs");
                if bin_name == name && main.exists() {
                    main
                } else {
                    dir.join("src/bin").join(format!("{bin_name}.rs"))
                }
            });
            bins.push(Bin { name: bin_name, path: bin_path });
        }
        if bins.is_empty() && dir.join("src/main.rs").exists() {
            bins.push(Bin { name: name.clone(), path: dir.join("src/main.rs") });
        }

        let build_script = match package.get("build") {
            Some(Value::Boolean(false)) => None,
            Some(Value::String(script)) => Some(dir.join(script)),
            _ => {
                let default = dir.join("build.rs");
                default.exists().then_some(default)
            }
        };

        let mut features = BTreeMap::new();
        for (feature, enables) in document.get("features").and_then(Value::as_table).into_iter().flatten() {
            features.insert(feature.clone(), enables.strings());
        }

        let mut dependencies = Vec::new();
        dependencies_of(document.get("dependencies"), Kind::Normal, None, dir, root.as_ref(), &mut dependencies)?;
        dependencies_of(
            document.get("build-dependencies").or_else(|| document.get("build_dependencies")),
            Kind::Build,
            None,
            dir,
            root.as_ref(),
            &mut dependencies,
        )?;
        for (platform, tables) in document.get("target").and_then(Value::as_table).into_iter().flatten() {
            dependencies_of(tables.get("dependencies"), Kind::Normal, Some(platform), dir, root.as_ref(), &mut dependencies)?;
            dependencies_of(
                tables.get("build-dependencies").or_else(|| tables.get("build_dependencies")),
                Kind::Build,
                Some(platform),
                dir,
                root.as_ref(),
                &mut dependencies,
            )?;
        }

        let mut metadata = BTreeMap::new();
        for key in ["description", "homepage", "repository", "license", "rust-version", "readme"] {
            if let Some(value) = package_field(package, root.as_ref(), key) {
                metadata.insert(key.to_string(), value);
            }
        }
        if let Some(authors) = package.get("authors") {
            metadata.insert(String::from("authors"), authors.strings().join(":"));
        }

        Ok(Manifest { dir: dir.to_path_buf(), name, version, edition, links, lib, bins, build_script, features, dependencies, metadata })
    }

    /// The optional dependencies a `dep:` entry names somewhere in the
    /// feature table: those have no implicit feature of their own name.
    pub fn explicit_dep_features(&self) -> std::collections::BTreeSet<String> {
        self.features
            .values()
            .flatten()
            .filter_map(|entry| entry.strip_prefix("dep:").map(String::from))
            .collect()
    }
}
