//! Which packages are compiled, with which features, for which side of the
//! build. A package is compiled once for the target and, when a build script
//! or a procedural macro needs it, once more for the host, each with only
//! the features that side asks for (Cargo's second resolver). Features are
//! activated the way a manifest declares them: a feature enables features,
//! optional dependencies (`dep:x`, or `x` when no `dep:x` names it) and
//! features of dependencies (`x/f`, and `x?/f` only once `x` is enabled).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::cfg::Target;
use crate::lock::{Lock, Package};
use crate::manifest::{Dependency, Kind, Manifest};

pub struct Node {
    pub package: Package,
    pub manifest: Manifest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Unit {
    pub node: usize,
    /// Compiled for the host: a build script, a procedural macro, or what they use.
    pub host: bool,
}

#[derive(Clone, Debug)]
pub struct Edge {
    pub alias: String,
    pub kind: Kind,
    pub to: Unit,
}

#[derive(Default)]
pub struct State {
    pub features: BTreeSet<String>,
    pub edges: Vec<Edge>,
    weak: Vec<(String, String)>,
}

pub struct Graph {
    pub nodes: Vec<Node>,
    pub units: BTreeMap<Unit, State>,
    pub root: Unit,
    /// For each node and each dependency line of its manifest, the node it is.
    targets: Vec<Vec<Option<usize>>>,
}

/// The semver major (or `0.minor`) a requirement or version names.
fn line_of(version: &str) -> String {
    let digits: String = version
        .trim_start_matches(|character: char| !character.is_ascii_digit())
        .chars()
        .take_while(|character| character.is_ascii_digit() || *character == '.')
        .collect();
    let mut parts = digits.split('.');
    let major = parts.next().unwrap_or("0");
    if major == "0" {
        format!("0.{}", parts.next().unwrap_or("0"))
    } else {
        major.to_string()
    }
}

/// The root's features as the build asks for them.
pub struct Features {
    /// Its `default` feature (off with `--no-default-features`).
    pub default: bool,
    /// Every feature it declares (`--all-features`).
    pub all: bool,
    /// The features `--features` names.
    pub named: Vec<String>,
}

impl Graph {
    pub fn build(
        lock: &Lock,
        root_name: &str,
        root_dir: &Path,
        cache: &Path,
        target: &Target,
        features: &Features,
    ) -> Result<Graph, String> {
        let mut nodes: Vec<Node> = Vec::new();
        let mut index: BTreeMap<String, usize> = BTreeMap::new();
        let root_package = lock.root(root_name)?.clone();
        let root_manifest = Manifest::read(root_dir)?;
        index.insert(root_package.id(), 0);
        nodes.push(Node { package: root_package, manifest: root_manifest });
        let mut targets: Vec<Vec<Option<usize>>> = Vec::new();
        let mut at = 0;
        while at < nodes.len() {
            let package = nodes[at].package.clone();
            let manifest = nodes[at].manifest.clone();
            let mut row = Vec::new();
            for dependency in &manifest.dependencies {
                if let Some(platform) = &dependency.platform {
                    if !target.matches(platform).map_err(|why| format!("{}: {why}", manifest.name))? {
                        row.push(None);
                        continue;
                    }
                }
                let candidates: Vec<&Package> = package
                    .dependencies
                    .iter()
                    .filter_map(|line| lock.resolve(line).ok())
                    .filter(|candidate| candidate.name == dependency.package)
                    .collect();
                let chosen = match candidates.as_slice() {
                    [] if dependency.optional => {
                        row.push(None);
                        continue;
                    }
                    [] => {
                        return Err(format!(
                            "{} depends on {}, and its lock entry names no such package",
                            package.id(),
                            dependency.package
                        ))
                    }
                    [only] => (*only).clone(),
                    several => {
                        let wanted = dependency.requirement.as_deref().map(line_of);
                        several
                            .iter()
                            .find(|candidate| Some(line_of(&candidate.version)) == wanted)
                            .map(|candidate| (*candidate).clone())
                            .ok_or_else(|| {
                                format!(
                                    "{} depends on {} {:?}, and none of its locked versions matches",
                                    package.id(),
                                    dependency.package,
                                    dependency.requirement
                                )
                            })?
                    }
                };
                let id = chosen.id();
                let position = match index.get(&id) {
                    Some(position) => *position,
                    None => {
                        let dir = match (&chosen.source, &dependency.path) {
                            (None, Some(path)) => path.clone(),
                            _ => crate::lock::source_dir(&chosen, root_dir, cache)?,
                        };
                        let manifest = Manifest::read(&dir)?;
                        nodes.push(Node { package: chosen, manifest });
                        index.insert(id, nodes.len() - 1);
                        nodes.len() - 1
                    }
                };
                row.push(Some(position));
            }
            targets.push(row);
            at += 1;
        }
        let root = Unit { node: 0, host: false };
        let mut graph = Graph { nodes, units: BTreeMap::new(), root, targets };
        graph.ensure(root)?;
        let declared: Vec<String> = graph.nodes[0].manifest.features.keys().cloned().collect();
        if features.default && declared.iter().any(|name| name == "default") {
            graph.activate(root, "default")?;
        }
        let asked: Vec<String> = if features.all { declared } else { features.named.clone() };
        for feature in asked {
            graph.activate(root, &feature).map_err(|why| format!("--features {feature}: {why}"))?;
        }
        Ok(graph)
    }

    fn manifest(&self, unit: Unit) -> &Manifest {
        &self.nodes[unit.node].manifest
    }

    /// The unit a dependency edge of `from` reaches.
    fn reached(&self, from: Unit, dependency: &Dependency, node: usize) -> Unit {
        let proc_macro = self.nodes[node].manifest.lib.as_ref().is_some_and(|lib| lib.proc_macro);
        let host = from.host || dependency.kind == Kind::Build || proc_macro;
        Unit { node, host }
    }

    fn ensure(&mut self, unit: Unit) -> Result<(), String> {
        if self.units.contains_key(&unit) {
            return Ok(());
        }
        self.units.insert(unit, State::default());
        let dependencies = self.manifest(unit).dependencies.clone();
        for (position, dependency) in dependencies.iter().enumerate() {
            if dependency.optional {
                continue;
            }
            if dependency.kind == Kind::Build && self.manifest(unit).build_script.is_none() {
                continue;
            }
            self.enable_line(unit, position, None)?;
        }
        Ok(())
    }

    /// Enable dependency line `position` of `unit`'s manifest, with one
    /// more feature of it when `extra` names one.
    fn enable_line(&mut self, unit: Unit, position: usize, extra: Option<&str>) -> Result<(), String> {
        let Some(node) = self.targets[unit.node][position] else { return Ok(()) };
        let dependency = self.manifest(unit).dependencies[position].clone();
        let to = self.reached(unit, &dependency, node);
        self.ensure(to)?;
        let state = self.units.get_mut(&unit).ok_or("a unit vanished")?;
        if !state.edges.iter().any(|edge| edge.alias == dependency.alias && edge.kind == dependency.kind && edge.to == to) {
            state.edges.push(Edge { alias: dependency.alias.clone(), kind: dependency.kind, to });
        }
        if dependency.default_features && self.nodes[node].manifest.features.contains_key("default") {
            self.activate(to, "default")?;
        }
        for feature in &dependency.features {
            self.activate(to, feature)?;
        }
        if let Some(feature) = extra {
            self.activate(to, feature)?;
        }
        // A weak feature waiting on this dependency applies now.
        let waiting: Vec<(String, String)> = {
            let state = self.units.get_mut(&unit).ok_or("a unit vanished")?;
            let (now, later): (Vec<_>, Vec<_>) = state.weak.drain(..).partition(|(alias, _)| *alias == dependency.alias);
            state.weak = later;
            now
        };
        for (_, feature) in waiting {
            self.activate(to, &feature)?;
        }
        Ok(())
    }

    /// The manifest lines of `unit` whose alias is `alias`.
    fn lines(&self, unit: Unit, alias: &str) -> Vec<usize> {
        let manifest = self.manifest(unit);
        manifest
            .dependencies
            .iter()
            .enumerate()
            .filter(|(_, dependency)| dependency.alias == alias)
            .filter(|(_, dependency)| dependency.kind == Kind::Normal || manifest.build_script.is_some())
            .map(|(position, _)| position)
            .collect()
    }

    fn enabled(&self, unit: Unit, alias: &str) -> bool {
        self.units.get(&unit).is_some_and(|state| state.edges.iter().any(|edge| edge.alias == alias))
    }

    pub fn activate(&mut self, unit: Unit, feature: &str) -> Result<(), String> {
        self.ensure(unit)?;
        let state = self.units.get_mut(&unit).ok_or("a unit vanished")?;
        if !state.features.insert(feature.to_string()) {
            return Ok(());
        }
        let manifest = self.manifest(unit).clone();
        if let Some(enables) = manifest.features.get(feature) {
            for entry in enables {
                if let Some(alias) = entry.strip_prefix("dep:") {
                    for line in self.lines(unit, alias) {
                        self.enable_line(unit, line, None)?;
                    }
                } else if let Some((alias, sub)) = entry.split_once('/') {
                    let (alias, weak) = match alias.strip_suffix('?') {
                        Some(alias) => (alias, true),
                        None => (alias, false),
                    };
                    if weak && !self.enabled(unit, alias) {
                        let state = self.units.get_mut(&unit).ok_or("a unit vanished")?;
                        state.weak.push((alias.to_string(), sub.to_string()));
                        continue;
                    }
                    let implicit = manifest.dependencies.iter().any(|dependency| dependency.alias == alias && dependency.optional)
                        && !manifest.explicit_dep_features().contains(alias);
                    if !weak && implicit {
                        let state = self.units.get_mut(&unit).ok_or("a unit vanished")?;
                        state.features.insert(alias.to_string());
                    }
                    for line in self.lines(unit, alias) {
                        self.enable_line(unit, line, Some(sub))?;
                    }
                } else {
                    self.activate(unit, entry)?;
                }
            }
            return Ok(());
        }
        let optional = manifest.dependencies.iter().any(|dependency| dependency.alias == feature && dependency.optional);
        if optional && !manifest.explicit_dep_features().contains(feature) {
            for line in self.lines(unit, feature) {
                self.enable_line(unit, line, None)?;
            }
            return Ok(());
        }
        Err(format!("{} {} has no feature {feature}", manifest.name, manifest.version))
    }

    /// Every unit, each after every unit it depends on.
    pub fn order(&self) -> Vec<Unit> {
        let mut done = BTreeSet::new();
        let mut out = Vec::new();
        let mut stack: Vec<(Unit, bool)> = vec![(self.root, false)];
        while let Some((unit, expanded)) = stack.pop() {
            if done.contains(&unit) {
                continue;
            }
            if expanded {
                done.insert(unit);
                out.push(unit);
                continue;
            }
            stack.push((unit, true));
            for edge in self.units.get(&unit).map(|state| state.edges.as_slice()).unwrap_or_default() {
                if !done.contains(&edge.to) {
                    stack.push((edge.to, false));
                }
            }
        }
        out
    }
}
