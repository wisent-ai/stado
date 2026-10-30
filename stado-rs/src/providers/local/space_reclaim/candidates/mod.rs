//! What holds this machine's disk, ranked as the next cleanup candidates:
//! `stado space candidates`.
//!
//! The disk-full rule ([`super::rule`]) says when the janitor deletes and
//! what it may take; it never said what else holds a full disk. An operator
//! at 80% found that out by hand, deleting tens of GiB of Cargo build
//! directories and an agent's adopted build tree himself. This module
//! measures the home once and ranks every reclaimable item in three classes,
//! in the order a cleanup should take them:
//!
//! 1. [`Class::AgentOutput`]: what builds and agents produced and the next
//!    build makes again — tagged build trees (`CACHEDIR.TAG`), Cargo's
//!    per-workspace build directories, job work trees, harness logs,
//!    delivered release copies;
//! 2. [`Class::Cache`]: what a tool downloaded or compiled and fetches or
//!    compiles again — the compiler cache, model downloads, package caches;
//! 3. [`Class::UserData`]: everything else at least the size asked for, a
//!    downloaded game among it. These are proposals only: nothing in Stado
//!    deletes them.
//!
//! Each row says whether the janitor's own pass takes it at the threshold
//! (`automatic`) or only proposes it, who produced it and how it comes back.
//! [`ledger`] is the record of what the janitor did take.

pub mod ledger;

use std::collections::BTreeSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::build_caches::{cargo_build_root, privacy_protected_parts, CARGO_BUILD_DIRS};
use super::{agent_logs, compiler_cache, delivered_releases, queue_workdirs, STATE_DIR_PARTS};
use crate::deploy::host_build_caches::CACHEDIR_SIGNATURE;

/// The three classes, in the order a cleanup takes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    AgentOutput,
    Cache,
    UserData,
}

impl Class {
    pub fn word(self) -> &'static str {
        match self {
            Self::AgentOutput => "agent_output",
            Self::Cache => "cache",
            Self::UserData => "user_data",
        }
    }
}

/// How a removed item comes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Safety {
    /// The next build makes it again from what is on the disk.
    Rebuildable,
    /// The tool downloads it again; it needs the network.
    Refetchable,
    /// Throwaway state builds and agents keep in a checkout: made again by
    /// the next build, but nothing proves no work still reads it.
    Scratch,
    /// Nothing makes it again: only its owner may decide.
    UserOwned,
}

impl Safety {
    pub fn word(self) -> &'static str {
        match self {
            Self::Rebuildable => "rebuildable",
            Self::Refetchable => "refetchable",
            Self::Scratch => "scratch",
            Self::UserOwned => "user_owned",
        }
    }
}

/// One fixed area a tool or Stado keeps under the home.
struct Area {
    /// Home-relative path.
    root: &'static str,
    class: Class,
    safety: Safety,
    producer: &'static str,
    /// The janitor cleaner that takes it at the threshold, if one does.
    cleaner: Option<&'static str>,
    /// What the janitor keeps of it, or why it proposes it only.
    note: &'static str,
}

/// Every fixed area, measured as one item each.
const AREAS: &[Area] = &[
    Area {
        root: queue_workdirs::WORK_ROOT,
        class: Class::AgentOutput,
        safety: Safety::Rebuildable,
        producer: "Stado queue jobs (work trees)",
        cleaner: Some(queue_workdirs::CLEANER),
        note: "the janitor takes the tree of every job the queue reports finished",
    },
    Area {
        root: ".stado/build-work",
        class: Class::AgentOutput,
        safety: Safety::Rebuildable,
        producer: "Stado release builds (scratch)",
        cleaner: None,
        note: "`stado space reclaim <host> --stage build_scratch` takes every tree no process or unit names",
    },
    Area {
        root: delivered_releases::DELIVERED_ROOT,
        class: Class::AgentOutput,
        safety: Safety::Rebuildable,
        producer: "Stado release delivery (staged copies)",
        cleaner: Some(delivered_releases::CLEANER),
        note: "the janitor keeps each product's installed and newest version",
    },
    Area {
        root: compiler_cache::STORE_ROOTS[0],
        class: Class::Cache,
        safety: Safety::Rebuildable,
        producer: "Kache compiler cache",
        cleaner: Some(compiler_cache::CLEANER),
        note: "the janitor empties it while no job runs",
    },
    Area {
        root: compiler_cache::STORE_ROOTS[1],
        class: Class::Cache,
        safety: Safety::Rebuildable,
        producer: "Kache compiler cache",
        cleaner: Some(compiler_cache::CLEANER),
        note: "the janitor empties it while no job runs",
    },
    Area {
        root: ".cache/huggingface/hub",
        class: Class::Cache,
        safety: Safety::Refetchable,
        producer: "Hugging Face model downloads",
        cleaner: Some("huggingface_cache"),
        note: "the janitor takes model blobs while no job runs",
    },
    Area {
        root: ".npm/_cacache",
        class: Class::Cache,
        safety: Safety::Refetchable,
        producer: "npm package cache",
        cleaner: None,
        note: "`npm cache clean --force` empties it",
    },
    Area {
        root: ".bun/install/cache",
        class: Class::Cache,
        safety: Safety::Refetchable,
        producer: "Bun package cache",
        cleaner: None,
        note: "`bun pm cache rm` empties it",
    },
    Area {
        root: ".cache/pip",
        class: Class::Cache,
        safety: Safety::Refetchable,
        producer: "pip package cache",
        cleaner: None,
        note: "`pip cache purge` empties it",
    },
    Area {
        root: "Library/Caches/pip",
        class: Class::Cache,
        safety: Safety::Refetchable,
        producer: "pip package cache",
        cleaner: None,
        note: "`pip cache purge` empties it",
    },
    Area {
        root: ".cache/uv",
        class: Class::Cache,
        safety: Safety::Refetchable,
        producer: "uv package cache",
        cleaner: None,
        note: "`uv cache clean` empties it",
    },
    Area {
        root: ".gradle/caches",
        class: Class::Cache,
        safety: Safety::Refetchable,
        producer: "Gradle dependency cache",
        cleaner: None,
        note: "Gradle fetches it again on the next build",
    },
    Area {
        root: ".m2/repository",
        class: Class::Cache,
        safety: Safety::Refetchable,
        producer: "Maven repository cache",
        cleaner: None,
        note: "Maven fetches it again on the next build",
    },
    Area {
        root: "Library/Caches/Homebrew",
        class: Class::Cache,
        safety: Safety::Refetchable,
        producer: "Homebrew download cache",
        cleaner: None,
        note: "`brew cleanup --prune=all` empties it",
    },
    Area {
        root: "Library/Caches/org.swift.swiftpm",
        class: Class::Cache,
        safety: Safety::Refetchable,
        producer: "SwiftPM package cache",
        cleaner: None,
        note: "SwiftPM fetches it again on the next resolve",
    },
    // Under `~/Library`, which no walk of the home opens: a downloaded game
    // lands here, and is the owner's to keep or delete.
    Area {
        root: "Library/Application Support/Steam/steamapps",
        class: Class::UserData,
        safety: Safety::UserOwned,
        producer: "Steam games the owner installed",
        cleaner: None,
        note: "the owner's games; Steam uninstalls one, and nothing in Stado deletes them",
    },
    Area {
        root: "Library/Caches/ms-playwright",
        class: Class::Cache,
        safety: Safety::Refetchable,
        producer: "Playwright browsers",
        cleaner: None,
        note: "`npx playwright install` fetches them again",
    },
    Area {
        root: ".cache/ms-playwright",
        class: Class::Cache,
        safety: Safety::Refetchable,
        producer: "Playwright browsers",
        cleaner: None,
        note: "`npx playwright install` fetches them again",
    },
];

/// Cargo's downloaded inputs, under Cargo's home rather than a fixed path.
/// `git/checkouts` is the package-cache tier's (Cargo checks each out again
/// from `git/db`); the registry and `git/db` are proposals.
const CARGO_INPUTS: &[(&str, &str)] = &[
    ("registry", "Cargo package registry"),
    ("git/db", "Cargo git dependencies (fetched repositories)"),
    ("git/checkouts", "Cargo git dependencies (checkouts)"),
];

/// Why the janitor keeps Cargo's fetched inputs.
const CARGO_INPUT_NOTE: &str = "Cargo's downloaded inputs: the janitor keeps them, since a build \
     running while they are deleted fails; `cargo cache --autoclean` or deleting them between \
     builds takes them";

/// Stado's own state under the home. Its reclaimable areas are listed above;
/// the rest (its binaries, configuration, registry copy, queue store) is not
/// a candidate in any class.
const STADO_TREE: &str = ".stado";

/// One ranked row.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub path: PathBuf,
    pub bytes: i64,
    pub class: Class,
    pub safety: Safety,
    pub producer: String,
    /// The janitor cleaner that takes it at the threshold; `None` when the
    /// row is a proposal.
    pub cleaner: Option<&'static str>,
    pub note: String,
}

/// A directory the survey did not measure, and why.
#[derive(Debug, Clone)]
pub struct Unmeasured {
    pub path: PathBuf,
    pub reason: String,
}

/// What the survey needs to know about this host.
pub struct Inputs<'a> {
    pub home: &'a Path,
    /// The consent-gated folders this program may open
    /// (`targets[].privacy_grants`).
    pub admitted: &'a BTreeSet<String>,
    /// The one tree the janitor deletes in on a shared host; `None` when it
    /// deletes in the whole home.
    pub deletable: Option<&'a Path>,
    /// The smallest item listed; smaller ones are counted in the totals.
    pub min_bytes: i64,
}

/// Every item, ranked, with the class totals and what was not measured.
pub struct Survey {
    pub candidates: Vec<Candidate>,
    pub totals: [(Class, i64, i64); 3],
    pub unmeasured: Vec<Unmeasured>,
}

/// Bytes a file occupies on the disk: its allocated blocks.
fn allocated(info: &std::fs::Metadata) -> i64 {
    i64::try_from(info.blocks()).map_or(i64::MAX, |blocks| blocks.saturating_mul(512))
}

/// Whether `dir` carries a `CACHEDIR.TAG` beginning with the standard's
/// signature, read without following a link.
fn tagged(dir: &Path) -> bool {
    let tag = dir.join("CACHEDIR.TAG");
    match std::fs::symlink_metadata(&tag) {
        Ok(info) if info.is_file() => {}
        _ => return false,
    }
    let mut head = vec![0u8; CACHEDIR_SIGNATURE.len()];
    std::fs::File::open(&tag)
        .and_then(|mut file| std::io::Read::read_exact(&mut file, &mut head))
        .is_ok_and(|()| head == CACHEDIR_SIGNATURE.as_bytes())
}

/// The build areas a checkout keeps beside its `.git`, which the workshop
/// gives agents and builds for throwaway state: `.build`, `.build-*` (an
/// agent's adopted or side build tree among them) and `target`, by the tool
/// that writes each. `None` for anything else, and for a directory whose
/// parent is not a Git checkout.
fn checkout_build_area(parent: &Path, name: &std::ffi::OsStr) -> Option<&'static str> {
    let name = name.to_str()?;
    let producer = if name == "target" {
        "Cargo (a checkout's target directory)"
    } else if name == ".build" {
        "builds and agents (a checkout's .build area)"
    } else if name.starts_with(".build-") {
        "an agent's side build tree in a checkout"
    } else {
        return None;
    };
    parent.join(".git").exists().then_some(producer)
}

/// A directory retained by the walk: at least the listed size.
struct Node {
    path: PathBuf,
    bytes: i64,
    children: Vec<Node>,
    files: Vec<(PathBuf, i64)>,
}

struct Walker<'a> {
    inputs: &'a Inputs<'a>,
    device: u64,
    /// Measured as their own rows, or not opened: never walked again.
    excluded: Vec<PathBuf>,
    candidates: Vec<Candidate>,
    unmeasured: Vec<Unmeasured>,
    /// Bytes of each class's rows below the listed size, by class.
    small: [i64; 3],
    /// Bytes of the user areas measured outside the walk (a game library
    /// under `~/Library`).
    user_area_bytes: i64,
}

impl Walker<'_> {
    fn automatic(
        &self,
        path: &Path,
        cleaner: Option<&'static str>,
        note: &str,
    ) -> (Option<&'static str>, String) {
        match cleaner {
            Some(_) if !self.inputs.deletable.is_none_or(|root| path.starts_with(root)) => (
                None,
                format!(
                    "{note}; on this shared host the janitor deletes only inside {}, so this is a \
                     proposal",
                    self.inputs.deletable.map_or_else(String::new, |root| root.display().to_string())
                ),
            ),
            cleaner => (cleaner, note.to_string()),
        }
    }

    fn push(&mut self, mut row: Candidate) {
        // A package manager's download cache is the janitor's second tier:
        // taken automatically, but only when agent output alone leaves the
        // volume at the rule.
        let home = self.inputs.home;
        if row.cleaner.is_none()
            && super::package_caches::CACHES
                .iter()
                .any(|cache| super::package_caches::cache_path(home, cache) == row.path)
        {
            let (cleaner, note) = self.automatic(
                &row.path,
                Some(super::package_caches::CLEANER),
                "a package manager's download cache; the janitor takes it, largest first, only \
                 when the agent output it took left the volume at the rule, and never while a \
                 running process holds it",
            );
            row.cleaner = cleaner;
            row.note = note;
        }
        if row.bytes >= self.inputs.min_bytes {
            self.candidates.push(row);
        } else {
            let slot = &mut self.small[row.class as usize];
            *slot = slot.saturating_add(row.bytes);
        }
    }

    /// Every byte under `dir`, on this volume, never following a link.
    fn plain_bytes(&mut self, dir: &Path) -> i64 {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) => {
                self.unmeasured.push(Unmeasured {
                    path: dir.to_path_buf(),
                    reason: format!("it cannot be listed: {error}"),
                });
                return 0;
            }
        };
        let mut total = 0i64;
        for entry in entries.flatten() {
            let Ok(info) = entry.metadata() else { continue };
            if info.is_dir() {
                if info.dev() == self.device {
                    total = total.saturating_add(self.plain_bytes(&entry.path()));
                }
            } else if !info.file_type().is_symlink() {
                total = total.saturating_add(allocated(&info));
            }
        }
        total
    }

    /// The trees registered in the scratch register, each its own row: the
    /// janitor takes one once its owner session has ended.
    fn registered(&mut self) {
        let states = match super::registered_scratch::states(self.inputs.home) {
            Ok(states) => states,
            Err(error) => {
                self.unmeasured.push(Unmeasured {
                    path: super::registered_scratch::register_path(self.inputs.home),
                    reason: error.to_string(),
                });
                return;
            }
        };
        for state in states {
            let Some(bytes) = state.bytes else { continue };
            self.excluded.push(state.entry.path.clone());
            let note = if state.owner_running {
                format!(
                    "registered by {} ({}); kept while its owner session {} runs",
                    state.entry.creator, state.entry.purpose, state.entry.owner_session
                )
            } else {
                format!(
                    "registered by {} ({}); its owner session {} has ended, so the janitor takes \
                     it at the rule",
                    state.entry.creator, state.entry.purpose, state.entry.owner_session
                )
            };
            self.push(Candidate {
                path: state.entry.path,
                bytes,
                class: Class::AgentOutput,
                safety: Safety::Scratch,
                producer: state.entry.creator,
                cleaner: Some(super::registered_scratch::CLEANER),
                note,
            });
        }
    }

    /// The fixed areas, Cargo's inputs and its build directories, each its
    /// own row; every one becomes a root the home walk does not enter.
    fn areas(&mut self) {
        let home = self.inputs.home;
        for area in AREAS {
            let path = home.join(area.root);
            self.excluded.push(path.clone());
            if !path.is_dir() {
                continue;
            }
            let bytes = self.plain_bytes(&path);
            if area.class == Class::UserData {
                self.user_area_bytes = self.user_area_bytes.saturating_add(bytes);
            }
            let (cleaner, note) = self.automatic(&path, area.cleaner, area.note);
            self.push(Candidate {
                path,
                bytes,
                class: area.class,
                safety: area.safety,
                producer: area.producer.to_string(),
                cleaner,
                note,
            });
        }
        for root in agent_logs::LOG_ROOTS {
            let path = home.join(root);
            self.excluded.push(path.clone());
            if !path.is_dir() {
                continue;
            }
            let bytes = self.plain_bytes(&path);
            let (cleaner, note) = self.automatic(
                &path,
                Some(agent_logs::CLEANER),
                "the janitor deletes every log file in it",
            );
            self.push(Candidate {
                path,
                bytes,
                class: Class::AgentOutput,
                safety: Safety::Rebuildable,
                producer: "coding-agent harness logs".to_string(),
                cleaner,
                note,
            });
        }
        // Xcode's per-project build directories: `~/Library` is no walk's,
        // so each is listed from its fixed root.
        let derived = home.join(super::xcode_derived_data::ROOT);
        if let Ok(projects) = std::fs::read_dir(&derived) {
            for project in projects.flatten() {
                if !project
                    .file_type()
                    .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
                {
                    continue;
                }
                let path = project.path();
                let bytes = self.plain_bytes(&path);
                let automatic = project
                    .file_name()
                    .to_str()
                    .is_some_and(super::xcode_derived_data::project_directory);
                let (cleaner, note) = if automatic {
                    self.automatic(
                        &path,
                        Some(super::xcode_derived_data::CLEANER),
                        "one project's Xcode build directory; the janitor takes it whole unless a \
                         running process holds it",
                    )
                } else {
                    (
                        None,
                        "Xcode's shared module cache; Xcode builds it again".to_string(),
                    )
                };
                self.push(Candidate {
                    path,
                    bytes,
                    class: Class::AgentOutput,
                    safety: Safety::Rebuildable,
                    producer: "Xcode / xcodebuild (DerivedData)".to_string(),
                    cleaner,
                    note,
                });
            }
        }
        let build = cargo_build_root(home);
        if let Some(cargo) = build.parent() {
            // `git` is listed by its two parts below; its other files (a
            // lock, a cache index) are not the user's.
            self.excluded.push(cargo.join("git"));
            for (name, producer) in CARGO_INPUTS {
                let path = cargo.join(name);
                self.excluded.push(path.clone());
                if path.is_dir() {
                    let bytes = self.plain_bytes(&path);
                    self.push(Candidate {
                        path,
                        bytes,
                        class: Class::Cache,
                        safety: Safety::Refetchable,
                        producer: (*producer).to_string(),
                        cleaner: None,
                        note: CARGO_INPUT_NOTE.to_string(),
                    });
                }
            }
        }
        self.excluded.push(build.clone());
        let shards = match std::fs::read_dir(&build) {
            Ok(shards) => shards,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) => {
                self.unmeasured.push(Unmeasured {
                    path: build,
                    reason: format!("Cargo's build directory cannot be listed: {error}"),
                });
                return;
            }
        };
        for shard in shards.flatten() {
            if !shard.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let Ok(workspaces) = std::fs::read_dir(shard.path()) else {
                continue;
            };
            for workspace in workspaces.flatten() {
                if !workspace.file_type().is_ok_and(|kind| kind.is_dir()) {
                    continue;
                }
                let path = workspace.path();
                let bytes = self.plain_bytes(&path);
                let (cleaner, note) = self.automatic(
                    &path,
                    Some(CARGO_BUILD_DIRS),
                    "one workspace's Cargo build directory; the janitor takes it whole unless a \
                     build holds its lock",
                );
                self.push(Candidate {
                    path,
                    bytes,
                    class: Class::AgentOutput,
                    safety: Safety::Rebuildable,
                    producer: "cargo build (build-dir under Cargo's home)".to_string(),
                    cleaner,
                    note,
                });
            }
        }
    }

    /// Walk `dir`: a tagged tree is its own row, an excluded root is passed
    /// over, and every directory and file at least the listed size is kept
    /// for the user rows.
    fn walk(&mut self, dir: &Path) -> Node {
        let mut node = Node {
            path: dir.to_path_buf(),
            bytes: 0,
            children: Vec::new(),
            files: Vec::new(),
        };
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) => {
                self.unmeasured.push(Unmeasured {
                    path: dir.to_path_buf(),
                    reason: format!("it cannot be listed: {error}"),
                });
                return node;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if self.excluded.iter().any(|root| root == &path) {
                continue;
            }
            let Ok(info) = entry.metadata() else { continue };
            if info.file_type().is_symlink() {
                continue;
            }
            if !info.is_dir() {
                let bytes = allocated(&info);
                node.bytes = node.bytes.saturating_add(bytes);
                if bytes >= self.inputs.min_bytes {
                    node.files.push((path, bytes));
                }
                continue;
            }
            if info.dev() != self.device {
                self.unmeasured.push(Unmeasured {
                    path,
                    reason: "another volume is mounted here; its usage is not this volume's"
                        .to_string(),
                });
                continue;
            }
            if tagged(&path) {
                let bytes = self.plain_bytes(&path);
                let (cleaner, note) = self.automatic(
                    &path,
                    Some(super::build_caches::SCAN),
                    "its build tool tagged it regenerable; the janitor takes it unless a build \
                     holds its lock",
                );
                self.push(Candidate {
                    path,
                    bytes,
                    class: Class::AgentOutput,
                    safety: Safety::Rebuildable,
                    producer: "the build tool that wrote its CACHEDIR.TAG".to_string(),
                    cleaner,
                    note,
                });
                continue;
            }
            if let Some(producer) = checkout_build_area(dir, &entry.file_name()) {
                let bytes = self.plain_bytes(&path);
                self.push(Candidate {
                    path,
                    bytes,
                    class: Class::AgentOutput,
                    safety: Safety::Scratch,
                    producer: producer.to_string(),
                    cleaner: None,
                    note: "a checkout's ignored build area, where agents and builds keep their \
                           throwaway state; `stado product build-trees remove` takes its tagged \
                           trees, and the rest is a proposal because nothing proves no work \
                           still reads it"
                        .to_string(),
                });
                continue;
            }
            let child = self.walk(&path);
            node.bytes = node.bytes.saturating_add(child.bytes);
            if child.bytes >= self.inputs.min_bytes {
                node.children.push(child);
            }
        }
        node
    }

    /// The user rows of one retained directory: its retained children and
    /// files, and itself for what they leave; a directory with none is one
    /// row whole.
    fn user_rows(&mut self, node: Node) {
        let listed: i64 = node.children.iter().map(|child| child.bytes).sum::<i64>()
            + node.files.iter().map(|(_, bytes)| bytes).sum::<i64>();
        let rest = node.bytes - listed;
        let whole = node.children.is_empty() && node.files.is_empty();
        for (path, bytes) in node.files {
            self.push(user_row(path, bytes, "a file"));
        }
        for child in node.children {
            self.user_rows(child);
        }
        if whole {
            self.push(user_row(node.path, node.bytes, "a directory"));
        } else if rest >= self.inputs.min_bytes {
            self.push(user_row(
                node.path,
                rest,
                "the rest of this directory, beside the rows listed beneath it",
            ));
        }
    }
}

/// Who produced a row of the home no tool declared.
const USER_PRODUCER: &str = "the account's owner";

fn user_row(path: PathBuf, bytes: i64, what: &str) -> Candidate {
    Candidate {
        path,
        bytes,
        class: Class::UserData,
        safety: Safety::UserOwned,
        producer: USER_PRODUCER.to_string(),
        cleaner: None,
        note: format!(
            "{what} no tool declared regenerable; only its owner decides, and nothing in Stado \
             deletes it"
        ),
    }
}

/// The home's device, or why it cannot be surveyed. A home that is absent,
/// not a directory or cannot be listed is refused naming it: a ranking of
/// nothing would read as a disk with nothing to reclaim.
pub fn check_home(home: &Path) -> Result<u64, String> {
    let refused = |cause: String| {
        format!(
            "the home {} cannot be surveyed, so nothing on its volume can be ranked: {cause}; \
             HOME must name this account's home directory, readable by this account",
            home.display()
        )
    };
    let info = std::fs::metadata(home).map_err(|error| refused(error.to_string()))?;
    if !info.is_dir() {
        return Err(refused("it is not a directory".to_string()));
    }
    std::fs::read_dir(home).map_err(|error| refused(format!("it cannot be listed: {error}")))?;
    Ok(info.dev())
}

/// Measure the home and rank what holds it.
pub fn survey(inputs: &Inputs<'_>) -> Result<Survey, String> {
    let home = inputs.home;
    let device = check_home(home)?;
    let mut walker = Walker {
        inputs,
        device,
        excluded: Vec::new(),
        candidates: Vec::new(),
        unmeasured: Vec::new(),
        small: [0; 3],
        user_area_bytes: 0,
    };
    walker.registered();
    walker.areas();
    walker.excluded.push(home.join(STADO_TREE));
    walker
        .excluded
        .push(home.join(STATE_DIR_PARTS[0]).join(STATE_DIR_PARTS[1]));
    for part in privacy_protected_parts(cfg!(target_os = "macos"), inputs.admitted) {
        let path = home.join(part);
        if path.exists() {
            walker.unmeasured.push(Unmeasured {
                reason: format!(
                    "macOS asks its owner before a program reads ~/{part}, so the walk does not \
                     open it; declare it in this host's targets[].privacy_grants for this program \
                     to have it measured"
                ),
                path: path.clone(),
            });
        }
        walker.excluded.push(path);
    }
    let root = walker.walk(home);
    // Bytes of the home the walk measured outside every listed area: the
    // user's, whether or not one row is large enough to list.
    let walked_user_bytes = root.bytes;
    // The home itself is never one row: its children are.
    for (path, bytes) in root.files {
        walker.push(user_row(path, bytes, "a file"));
    }
    for child in root.children {
        walker.user_rows(child);
    }
    let mut candidates = walker.candidates;
    candidates.sort_by(|left, right| {
        left.class
            .cmp(&right.class)
            .then(right.bytes.cmp(&left.bytes))
            .then(left.path.cmp(&right.path))
    });
    // Each class's bytes: every row of it, listed or not. The user's are the
    // walk's measure of the home outside every listed area, plus the user
    // areas measured on their own.
    let mut totals = [
        (Class::AgentOutput, 0, walker.small[0]),
        (Class::Cache, 0, walker.small[1]),
        (
            Class::UserData,
            0,
            walked_user_bytes.saturating_add(walker.user_area_bytes),
        ),
    ];
    for row in &candidates {
        let slot = &mut totals[row.class as usize];
        slot.1 += 1;
        if row.class != Class::UserData {
            slot.2 = slot.2.saturating_add(row.bytes);
        }
    }
    Ok(Survey {
        candidates,
        totals,
        unmeasured: walker.unmeasured,
    })
}

/// The survey as JSON rows, each with the bytes the ranking frees up to and
/// including it.
pub fn rows_json(candidates: &[Candidate]) -> Vec<Value> {
    let mut cumulative = 0i64;
    candidates
        .iter()
        .map(|row| {
            cumulative = cumulative.saturating_add(row.bytes);
            json!({
                "path": row.path,
                "bytes": row.bytes,
                "class": row.class.word(),
                "safety": row.safety.word(),
                "producer": row.producer,
                "action": if row.cleaner.is_some() { "automatic" } else { "proposal" },
                "cleaner": row.cleaner,
                "note": row.note,
                "cumulative_bytes": cumulative,
            })
        })
        .collect()
}
