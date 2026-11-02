//! Compiling the resolved graph with `rustc`: every unit once, as many at a
//! time as the machine has cores, each as soon as what it needs exists.
//!
//! A library above another needs only the other's interface (its `.rmeta`),
//! which `rustc` writes long before it finishes the code, so a library
//! starts when the libraries under it have written their interfaces
//! (pipelining). A build script, a procedural macro and the binaries are
//! linked, so they wait for finished libraries.
//!
//! A unit's output is kept in the cache under a name derived from everything
//! that decides it (the package, its features, its side, the units under it,
//! the profile, the target and the compiler), so a unit whose inputs did not
//! change is not compiled again, by this package or by any other built with
//! the same cache. A package read from a path (the root and its workspace
//! neighbours) is reused only while every file its compilation read, as
//! `rustc` listed them, every file and variable its build script named, and
//! every variable the compiler read are what they were.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;

use crate::cfg::Target;
use crate::manifest::Kind;
use crate::profile::Profile;
use crate::resolve::{Graph, Unit};
use crate::script;

/// The marker of a variable that was not set.
const UNSET: &str = "\u{0}unset";

/// What one compiled unit leaves for the units above it.
#[derive(Clone, Debug, Default)]
pub struct Built {
    /// The library's crate name and finished file.
    pub lib: Option<(String, PathBuf)>,
    /// The library's interface, written before the code is finished.
    pub meta: Option<PathBuf>,
    /// Every `-L` its build script and the build scripts under it gave.
    pub searches: Vec<String>,
    /// Its build script's metadata, for packages with `links`.
    pub metadata: Vec<(String, String)>,
    /// For a path package: every file it read, with the digest it had.
    pub files: Vec<(String, PathBuf)>,
    /// For a path package: every variable it read, with the value it had.
    pub env: Vec<(String, String)>,
    /// Whether it was found already compiled.
    pub reused: bool,
}

impl Built {
    fn write(&self, path: &Path) -> Result<(), String> {
        let mut text = String::new();
        if let Some((name, file)) = &self.lib {
            text.push_str(&format!("lib\t{name}\t{}\n", file.display()));
        }
        if let Some(meta) = &self.meta {
            text.push_str(&format!("rmeta\t{}\n", meta.display()));
        }
        for search in &self.searches {
            text.push_str(&format!("search\t{search}\n"));
        }
        for (key, value) in &self.metadata {
            text.push_str(&format!("meta\t{key}\t{value}\n"));
        }
        for (digest, file) in &self.files {
            text.push_str(&format!("file\t{digest}\t{}\n", file.display()));
        }
        for (name, value) in &self.env {
            text.push_str(&format!("env\t{name}\t{value}\n"));
        }
        std::fs::write(path, text).map_err(|error| format!("{} cannot be written: {error}", path.display()))
    }

    fn read(path: &Path) -> Option<Built> {
        let text = std::fs::read_to_string(path).ok()?;
        let mut built = Built { reused: true, ..Built::default() };
        for line in text.lines() {
            let fields: Vec<&str> = line.splitn(3, '\t').collect();
            match fields.as_slice() {
                ["lib", name, file] => {
                    if !Path::new(file).exists() {
                        return None;
                    }
                    built.lib = Some((name.to_string(), PathBuf::from(file)));
                }
                ["rmeta", file] => built.meta = Some(PathBuf::from(file)),
                ["search", search] => built.searches.push(search.to_string()),
                ["meta", key, value] => built.metadata.push((key.to_string(), value.to_string())),
                ["file", digest, file] => built.files.push((digest.to_string(), PathBuf::from(file))),
                ["env", name, value] => built.env.push((name.to_string(), value.to_string())),
                _ => return None,
            }
        }
        Some(built)
    }

    /// What changed since a path package was compiled: the first file or
    /// variable that differs; none when everything is as it was.
    fn changed(&self) -> Option<String> {
        for (digest, file) in &self.files {
            match digest_of(file) {
                Some(now) if now == *digest => {}
                Some(_) => return Some(format!("{} changed", file.display())),
                None => return Some(format!("{} cannot be read any more", file.display())),
            }
        }
        self.env
            .iter()
            .find(|(name, value)| variable(name) != **value)
            .map(|(name, _)| format!("the variable {name} changed"))
    }
}

/// A file's digest; none when it cannot be read.
fn digest_of(file: &Path) -> Option<String> {
    let bytes = std::fs::read(file).ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    Some(format!("{:016x}", hasher.finish()))
}

fn variable(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| String::from(UNSET))
}

/// Every file and variable a dep-info file lists.
fn dep_info(path: &Path, files: &mut BTreeSet<PathBuf>, env: &mut BTreeMap<String, String>) {
    let Ok(text) = std::fs::read_to_string(path) else { return };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# env-dep:") {
            let name = rest.split_once('=').map_or(rest, |(name, _)| name);
            env.insert(name.to_string(), variable(name));
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        let Some((_, listed)) = line.split_once(": ").or_else(|| line.strip_suffix(':').map(|target| (target, ""))) else {
            continue;
        };
        let mut current = String::new();
        let mut chars = listed.chars().peekable();
        while let Some(character) = chars.next() {
            match character {
                '\\' if chars.peek() == Some(&' ') => {
                    current.push(' ');
                    chars.next();
                }
                ' ' => {
                    if !current.is_empty() {
                        files.insert(PathBuf::from(std::mem::take(&mut current)));
                    }
                }
                other => current.push(other),
            }
        }
        if !current.is_empty() {
            files.insert(PathBuf::from(current));
        }
    }
}

/// A digest of every file under a path package's directory, so a change to
/// its sources gives its unit, and every unit above it, a new identity.
fn tree_digest(dir: &Path, hasher: &mut impl Hasher) {
    let mut pending = vec![dir.to_path_buf()];
    let mut files = Vec::new();
    while let Some(here) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&here) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if !matches!(name.as_str(), "target" | ".build" | ".git" | "node_modules") {
                    pending.push(path);
                }
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    for file in files {
        file.hash(hasher);
        digest_of(&file).hash(hasher);
    }
}

pub struct Settings {
    pub rustc: String,
    pub rustc_version: String,
    pub profile: Profile,
    /// Compile path packages incrementally whatever the profile says.
    pub incremental: bool,
    pub jobs: usize,
    /// Where the binaries and the incremental state go.
    pub out: PathBuf,
    /// Where compiled units are kept, for this build and others.
    pub cache: PathBuf,
    /// The binaries of the root to link; every one when empty.
    pub bins: Vec<String>,
}

/// The units' identities: a digest of everything that decides each output.
fn identities(graph: &Graph, settings: &Settings, target: &Target, order: &[Unit]) -> BTreeMap<Unit, String> {
    let mut identities: BTreeMap<Unit, String> = BTreeMap::new();
    for unit in order {
        let node = &graph.nodes[unit.node];
        let state = &graph.units[unit];
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        node.package.id().hash(&mut hasher);
        node.package.source.hash(&mut hasher);
        unit.host.hash(&mut hasher);
        state.features.hash(&mut hasher);
        settings.profile.hash(&mut hasher);
        settings.incremental.hash(&mut hasher);
        settings.rustc_version.hash(&mut hasher);
        target.triple.hash(&mut hasher);
        if node.package.source.is_none() && *unit != graph.root {
            tree_digest(&node.manifest.dir, &mut hasher);
        }
        let mut edges: Vec<(String, String, String)> = state
            .edges
            .iter()
            .map(|edge| (edge.alias.clone(), format!("{:?}", edge.kind), identities.get(&edge.to).cloned().unwrap_or_default()))
            .collect();
        edges.sort();
        edges.hash(&mut hasher);
        identities.insert(*unit, format!("{:016x}", hasher.finish()));
    }
    identities
}

/// The value of the JSON string field `key` in one line `rustc` printed.
fn json_field(line: &str, key: &str) -> Option<String> {
    let marker = format!("\"{key}\":\"");
    let start = line.find(&marker)? + marker.len();
    let mut out = String::new();
    let mut chars = line[start..].chars();
    while let Some(character) = chars.next() {
        match character {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'u' => {
                    let code: String = chars.by_ref().take(4).collect();
                    out.push(u32::from_str_radix(&code, 16).ok().and_then(char::from_u32).unwrap_or('?'));
                }
                other => out.push(other),
            },
            other => out.push(other),
        }
    }
    None
}

/// Run `rustc`, reading its JSON messages and calling `on_meta` the moment
/// the interface is written; what it reported, or why it failed.
fn run_rustc(mut command: Command, what: &str, mut on_meta: impl FnMut(PathBuf)) -> Result<String, String> {
    command.arg("--error-format=json").arg("--json=artifacts,diagnostic-rendered-ansi");
    command.stderr(Stdio::piped()).stdout(Stdio::null());
    let mut child = command.spawn().map_err(|error| format!("rustc could not start for {what}: {error}"))?;
    let stderr = child.stderr.take().ok_or("rustc's error output could not be read")?;
    let mut rendered = String::new();
    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
        if line.contains("\"$message_type\":\"artifact\"") || line.starts_with("{\"artifact\"") {
            if json_field(&line, "emit").as_deref() == Some("metadata") {
                if let Some(path) = json_field(&line, "artifact") {
                    on_meta(PathBuf::from(path));
                }
            }
        } else if let Some(text) = json_field(&line, "rendered") {
            rendered.push_str(&text);
        } else if !line.starts_with('{') {
            rendered.push_str(&line);
            rendered.push('\n');
        }
    }
    let status = child.wait().map_err(|error| format!("rustc for {what} could not be waited for: {error}"))?;
    if !status.success() {
        return Err(format!("rustc failed for {what} ({status}):\n{}", rendered.trim()));
    }
    Ok(rendered)
}

/// The extern name a dependency edge gives its crate.
fn extern_name(alias: &str, package: &str, crate_name: &str) -> String {
    if alias == package {
        crate_name.to_string()
    } else {
        alias.replace('-', "_")
    }
}

/// What a unit tells the scheduler while it compiles.
enum Event {
    /// Its interface is written: libraries above it may start.
    Meta(Unit, Built),
    Done(Unit, Result<Built, String>, f64),
}

struct Job<'a> {
    graph: &'a Graph,
    target: &'a Target,
    settings: &'a Settings,
    identities: &'a BTreeMap<Unit, String>,
    deps: PathBuf,
}

/// The kind of output one `rustc` call makes.
#[derive(Clone, Copy, PartialEq)]
enum Output {
    Library,
    ProcMacro,
    Binary,
    BuildScript,
}

impl<'a> Job<'a> {
    fn local(&self, unit: Unit) -> bool {
        self.graph.nodes[unit.node].package.source.is_none()
    }

    fn base(&self, unit: Unit, output: Output, identity: &str, crate_name: &str, edition: &str, source: &Path) -> Command {
        let node = &self.graph.nodes[unit.node];
        let host = unit.host || matches!(output, Output::BuildScript | Output::ProcMacro);
        let profile = if host { &self.settings.profile.host } else { &self.settings.profile.target };
        let mut command = Command::new(&self.settings.rustc);
        command
            .arg("--crate-name")
            .arg(crate_name)
            .arg("--edition")
            .arg(edition)
            .arg(source)
            .arg("-C")
            .arg(format!("metadata={identity}"))
            .arg("-C")
            .arg(format!("extra-filename=-{identity}"))
            .arg("-L")
            .arg(format!("dependency={}", self.deps.display()));
        command.arg("-C").arg(format!("opt-level={}", profile.opt_level));
        if profile.debug != "0" {
            command.arg("-C").arg(format!("debuginfo={}", profile.debug));
        }
        command.arg("-C").arg(format!("debug-assertions={}", if profile.debug_assertions { "on" } else { "off" }));
        command.arg("-C").arg(format!("overflow-checks={}", if profile.overflow_checks { "on" } else { "off" }));
        if profile.panic == "abort" && matches!(output, Output::Library | Output::Binary) && !host {
            command.arg("-C").arg("panic=abort");
        }
        let incremental = self.local(unit) && (profile.incremental || self.settings.incremental);
        if incremental {
            command.arg("-C").arg(format!("incremental={}", self.settings.out.join("incremental").display()));
        } else if let Some(units) = profile.codegen_units {
            command.arg("-C").arg(format!("codegen-units={units}"));
        }
        match (output, profile.lto.as_str()) {
            (Output::Binary, "fat") => {
                command.arg("-C").arg("lto");
            }
            (Output::Binary, "thin") => {
                command.arg("-C").arg("lto=thin");
            }
            (Output::Library | Output::Binary | Output::BuildScript, "off") => {
                command.arg("-C").arg("embed-bitcode=no");
            }
            _ => {}
        }
        if output == Output::Binary && profile.strip != "none" {
            command.arg("-C").arg(format!("strip={}", profile.strip));
        }
        if !self.local(unit) {
            command.arg("--cap-lints").arg("allow");
        }
        for feature in &self.graph.units[&unit].features {
            command.arg("--cfg").arg(format!("feature=\"{feature}\""));
        }
        script::package_env(&mut command, &node.manifest);
        command.env("CARGO_MANIFEST_DIR", &node.manifest.dir);
        command.env("CARGO_MANIFEST_PATH", node.manifest.dir.join("Cargo.toml"));
        command.env("CARGO_CRATE_NAME", crate_name);
        if unit.node == self.graph.root.node {
            command.env("CARGO_PRIMARY_PACKAGE", "1");
        }
        command
    }

    /// Externs and search paths of the edges of `kind` of `unit`; with
    /// `interfaces`, a library's interface where its finished file is not
    /// needed.
    fn link_edges(&self, command: &mut Command, unit: Unit, kind: Kind, built: &BTreeMap<Unit, Built>, interfaces: bool) -> Vec<String> {
        let mut searches = BTreeSet::new();
        for edge in &self.graph.units[&unit].edges {
            if edge.kind != kind {
                continue;
            }
            let Some(below) = built.get(&edge.to) else { continue };
            searches.extend(below.searches.iter().cloned());
            if let Some((crate_name, file)) = &below.lib {
                let package = &self.graph.nodes[edge.to.node].manifest.name;
                let file = match (&below.meta, interfaces) {
                    (Some(meta), true) => meta,
                    _ => file,
                };
                command.arg("--extern").arg(format!("{}={}", extern_name(&edge.alias, package, crate_name), file.display()));
            }
        }
        searches.into_iter().collect()
    }

    /// Show what `rustc` reported for a package of this workspace.
    fn report(&self, unit: Unit, rendered: &str) {
        if self.local(unit) && !rendered.trim().is_empty() {
            eprint!("{rendered}");
        }
    }

    fn compile(&self, unit: Unit, built: &BTreeMap<Unit, Built>, events: &mpsc::Sender<Event>) -> Result<Built, String> {
        let node = &self.graph.nodes[unit.node];
        let manifest = &node.manifest;
        let identity = &self.identities[&unit];
        let unit_dir = self.settings.cache.join("build").join(format!("{}-{identity}", manifest.name));
        let record = unit_dir.join("built.txt");
        let root = unit == self.graph.root;
        let wanted_bins: Vec<&crate::manifest::Bin> = if root {
            manifest.bins.iter().filter(|bin| self.settings.bins.is_empty() || self.settings.bins.contains(&bin.name)).collect()
        } else {
            Vec::new()
        };
        if let Some(found) = Built::read(&record) {
            if !self.local(unit) {
                return Ok(found);
            }
            let missing = wanted_bins.iter().find(|bin| !self.settings.out.join(&bin.name).exists());
            match (found.changed(), missing) {
                (None, None) => return Ok(found),
                (Some(why), _) => eprintln!("compiling {} again: {why}", manifest.name),
                (None, Some(bin)) => eprintln!("compiling {} again: the binary {} is gone", manifest.name, bin.name),
            }
        }
        std::fs::create_dir_all(&unit_dir).map_err(|error| format!("{} cannot be made: {error}", unit_dir.display()))?;
        let mut files: BTreeSet<PathBuf> = BTreeSet::new();
        let mut env: BTreeMap<String, String> = BTreeMap::new();
        files.insert(manifest.dir.join("Cargo.toml"));

        // The build script: compiled with the build dependencies, then run.
        let mut output = script::Output::default();
        if let Some(build_script) = &manifest.build_script {
            let script_name = format!("build_script_{}", manifest.name.replace('-', "_"));
            let script_identity = format!("{identity}s");
            let mut command = self.base(unit, Output::BuildScript, &script_identity, &script_name, &manifest.edition, build_script);
            let script_info = unit_dir.join("build-script.d");
            command.arg("--crate-type").arg("bin").arg(format!("--emit=dep-info={},link", script_info.display()));
            let binary = unit_dir.join("build-script");
            command.arg("-o").arg(&binary);
            for search in self.link_edges(&mut command, unit, Kind::Build, built, false) {
                command.arg("-L").arg(search);
            }
            let rendered = run_rustc(command, &format!("the build script of {} {}", manifest.name, manifest.version), |_| {})?;
            self.report(unit, &rendered);
            dep_info(&script_info, &mut files, &mut env);
            let mut dep_metadata = BTreeMap::new();
            for edge in &self.graph.units[&unit].edges {
                if edge.kind != Kind::Normal {
                    continue;
                }
                let Some(links) = &self.graph.nodes[edge.to.node].manifest.links else { continue };
                let Some(below) = built.get(&edge.to) else { continue };
                for (key, value) in &below.metadata {
                    dep_metadata.insert(
                        format!("DEP_{}_{}", links.to_uppercase().replace('-', "_"), key.to_uppercase().replace('-', "_")),
                        value.clone(),
                    );
                }
            }
            let environment = script::Environment {
                manifest,
                out_dir: &unit_dir.join("out"),
                features: &self.graph.units[&unit].features,
                target: self.target,
                rustc: &self.settings.rustc,
                profile_release: self.settings.profile.name == "release",
                jobs: self.settings.jobs,
                dep_metadata: &dep_metadata,
            };
            output = script::run(&binary, &environment)?;
            if self.local(unit) {
                for warning in &output.warnings {
                    eprintln!("warning: {}: {warning}", manifest.name);
                }
            }
            for file in &output.rerun_files {
                files.insert(manifest.dir.join(file));
            }
            for name in &output.rerun_env {
                env.insert(name.clone(), variable(name));
            }
        }

        let mut result = Built { metadata: output.metadata.clone(), ..Built::default() };
        let out_dir = unit_dir.join("out");
        if let Some(lib) = &manifest.lib {
            let kind = if lib.proc_macro { Output::ProcMacro } else { Output::Library };
            let mut command = self.base(unit, kind, identity, &lib.crate_name, &lib.edition, &lib.path);
            let lib_info = unit_dir.join("lib.d");
            if lib.proc_macro {
                command
                    .arg("--crate-type")
                    .arg("proc-macro")
                    .arg("--extern")
                    .arg("proc_macro")
                    .arg(format!("--emit=dep-info={},link", lib_info.display()));
            } else {
                command.arg("--crate-type").arg("lib").arg(format!("--emit=dep-info={},metadata,link", lib_info.display()));
            }
            command.arg("--out-dir").arg(&self.deps);
            let searches = self.link_edges(&mut command, unit, Kind::Normal, built, !lib.proc_macro);
            self.apply_script(&mut command, &output, manifest.build_script.is_some(), &out_dir);
            for search in &searches {
                command.arg("-L").arg(search);
            }
            let file_name = if lib.proc_macro {
                format!("lib{}-{identity}.{}", lib.crate_name, dylib_extension())
            } else {
                format!("lib{}-{identity}.rlib", lib.crate_name)
            };
            result.lib = Some((lib.crate_name.clone(), self.deps.join(file_name)));
            let mut all: BTreeSet<String> = searches.into_iter().collect();
            all.extend(output.searches.iter().cloned());
            result.searches = all.into_iter().collect();
            let what = format!("{} {}", manifest.name, manifest.version);
            let early = result.clone();
            let proc_macro = lib.proc_macro;
            let rendered = run_rustc(command, &what, |path| {
                if !proc_macro {
                    let _ = events.send(Event::Meta(unit, Built { meta: Some(path), ..early.clone() }));
                }
            })?;
            self.report(unit, &rendered);
            if !lib.proc_macro {
                result.meta = Some(self.deps.join(format!("lib{}-{identity}.rmeta", lib.crate_name)));
            }
            dep_info(&lib_info, &mut files, &mut env);
        } else {
            result.searches = output.searches.clone();
        }

        for bin in wanted_bins {
            let mut command = self.base(unit, Output::Binary, identity, &bin.name.replace('-', "_"), &manifest.edition, &bin.path);
            let bin_info = unit_dir.join(format!("bin-{}.d", bin.name));
            command.arg("--crate-type").arg("bin").arg(format!("--emit=dep-info={},link", bin_info.display()));
            std::fs::create_dir_all(&self.settings.out)
                .map_err(|error| format!("{} cannot be made: {error}", self.settings.out.display()))?;
            command.arg("-o").arg(self.settings.out.join(&bin.name));
            let mut searches: BTreeSet<String> = self.link_edges(&mut command, unit, Kind::Normal, built, false).into_iter().collect();
            searches.extend(result.searches.iter().cloned());
            if let Some((crate_name, file)) = &result.lib {
                command.arg("--extern").arg(format!("{crate_name}={}", file.display()));
            }
            self.apply_script(&mut command, &output, manifest.build_script.is_some(), &out_dir);
            for search in searches {
                command.arg("-L").arg(search);
            }
            for argument in &output.link_args {
                command.arg("-C").arg(format!("link-arg={argument}"));
            }
            let rendered = run_rustc(command, &format!("the binary {}", bin.name), |_| {})?;
            self.report(unit, &rendered);
            dep_info(&bin_info, &mut files, &mut env);
        }

        if self.local(unit) {
            // Files the compiler produced are its output, not its input.
            let produced = [&self.settings.cache, &self.settings.out];
            result.files = files
                .into_iter()
                .filter(|file| !produced.iter().any(|dir| file.starts_with(dir)))
                .filter_map(|file| digest_of(&file).map(|digest| (digest, file)))
                .collect();
            // Variables this build sets for the compiler itself (the
            // package's, OUT_DIR, the build script's rustc-env) follow from
            // inputs already kept; only the outside environment is compared.
            let own: BTreeSet<&str> = output.envs.iter().map(|(name, _)| name.as_str()).collect();
            result.env = env
                .into_iter()
                .filter(|(name, _)| !name.starts_with("CARGO_") && name != "OUT_DIR" && !own.contains(name.as_str()))
                .collect();
        }
        result.write(&record)?;
        Ok(result)
    }

    fn apply_script(&self, command: &mut Command, output: &script::Output, has_script: bool, out_dir: &Path) {
        if has_script {
            command.env("OUT_DIR", out_dir);
        }
        for cfg in &output.cfgs {
            command.arg("--cfg").arg(cfg);
        }
        for (key, value) in &output.envs {
            command.env(key, value);
        }
        for lib in &output.libs {
            command.arg("-l").arg(lib);
        }
        for search in &output.searches {
            command.arg("-L").arg(search);
        }
    }
}

fn dylib_extension() -> &'static str {
    if cfg!(target_os = "macos") {
        "dylib"
    } else if cfg!(windows) {
        "dll"
    } else {
        "so"
    }
}

/// What a unit waits for before it starts.
struct Waits {
    /// Libraries whose interface is enough.
    interface: BTreeSet<Unit>,
    /// Units that must be finished: everything a link reads. A finished
    /// library may have been compiled against interfaces only, so a link
    /// waits for the whole closure of libraries under what it links.
    finished: BTreeSet<Unit>,
}

fn is_proc_macro(graph: &Graph, unit: Unit) -> bool {
    graph.nodes[unit.node].manifest.lib.as_ref().is_some_and(|lib| lib.proc_macro)
}

/// `from` and every library under it through normal dependencies.
fn linked_closure(graph: &Graph, from: impl IntoIterator<Item = Unit>, into: &mut BTreeSet<Unit>) {
    let mut pending: Vec<Unit> = from.into_iter().collect();
    while let Some(unit) = pending.pop() {
        if !into.insert(unit) {
            continue;
        }
        for edge in &graph.units[&unit].edges {
            if edge.kind == Kind::Normal {
                pending.push(edge.to);
            }
        }
    }
}

/// Compile every unit of `graph`; how many were compiled and how many reused.
pub fn build(graph: &Graph, target: &Target, settings: &Settings) -> Result<(usize, usize), String> {
    let order = graph.order();
    let identities = identities(graph, settings, target, &order);
    let deps = settings.cache.join("deps");
    std::fs::create_dir_all(&deps).map_err(|error| format!("{} cannot be made: {error}", deps.display()))?;

    let mut waits: BTreeMap<Unit, Waits> = BTreeMap::new();
    for unit in &order {
        let mut wait = Waits { interface: BTreeSet::new(), finished: BTreeSet::new() };
        if *unit == graph.root {
            wait.finished.extend(order.iter().filter(|other| *other != unit).copied());
        } else {
            let edges = &graph.units[unit].edges;
            // A build script links its build dependencies.
            linked_closure(graph, edges.iter().filter(|edge| edge.kind == Kind::Build).map(|edge| edge.to), &mut wait.finished);
            if is_proc_macro(graph, *unit) {
                // A procedural macro is linked: every library under it.
                linked_closure(graph, edges.iter().filter(|edge| edge.kind == Kind::Normal).map(|edge| edge.to), &mut wait.finished);
            } else {
                for edge in edges.iter().filter(|edge| edge.kind == Kind::Normal) {
                    if is_proc_macro(graph, edge.to) {
                        wait.finished.insert(edge.to);
                    } else {
                        wait.interface.insert(edge.to);
                    }
                }
            }
        }
        waits.insert(*unit, wait);
    }
    let mut interfaced: BTreeSet<Unit> = BTreeSet::new();
    let mut done: BTreeSet<Unit> = BTreeSet::new();
    let mut started: BTreeSet<Unit> = BTreeSet::new();
    let mut built: BTreeMap<Unit, Built> = BTreeMap::new();
    let (sender, receiver) = mpsc::channel::<Event>();
    let job = Job { graph, target, settings, identities: &identities, deps };
    let total = order.len();
    let mut compiled = 0;
    let mut reused = 0;
    let mut failures = Vec::new();
    std::thread::scope(|scope| {
        let mut running = 0usize;
        loop {
            if failures.is_empty() {
                for unit in &order {
                    if running >= settings.jobs {
                        break;
                    }
                    if started.contains(unit) {
                        continue;
                    }
                    let wait = &waits[unit];
                    let ready = wait.finished.iter().all(|below| done.contains(below))
                        && wait.interface.iter().all(|below| interfaced.contains(below) || done.contains(below));
                    if !ready {
                        continue;
                    }
                    started.insert(*unit);
                    running += 1;
                    let sender = sender.clone();
                    let snapshot = built.clone();
                    let job = &job;
                    let unit = *unit;
                    scope.spawn(move || {
                        let began = std::time::Instant::now();
                        let result = job.compile(unit, &snapshot, &sender);
                        let _ = sender.send(Event::Done(unit, result, began.elapsed().as_secs_f64()));
                    });
                }
            }
            if running == 0 {
                break;
            }
            let Ok(event) = receiver.recv() else { break };
            match event {
                Event::Meta(unit, early) => {
                    interfaced.insert(unit);
                    built.entry(unit).or_insert(early);
                }
                Event::Done(unit, result, seconds) => {
                    running -= 1;
                    match result {
                        Ok(finished) => {
                            if finished.reused {
                                reused += 1;
                            } else {
                                compiled += 1;
                                let node = &graph.nodes[unit.node];
                                eprintln!(
                                    "compiled {} {}{} in {seconds:.1}s ({}/{total})",
                                    node.package.name,
                                    node.package.version,
                                    if unit.host { " for the host" } else { "" },
                                    done.len() + 1
                                );
                            }
                            built.insert(unit, finished);
                            interfaced.insert(unit);
                            done.insert(unit);
                        }
                        Err(why) => failures.push(why),
                    }
                }
            }
        }
    });
    if !failures.is_empty() {
        return Err(failures.join("\n\n"));
    }
    if done.len() != total {
        return Err(format!("{} of {total} units were compiled; the others wait on a failed unit", done.len()));
    }
    Ok((compiled, reused))
}
