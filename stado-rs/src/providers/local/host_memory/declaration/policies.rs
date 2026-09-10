//! The fleet's declared memory policies: whole `memory_reclaim` documents,
//! named, reviewed in git, and applied to a host by name.
//!
//! The sibling document `memory/repairs.json` declares which repairs EXIST.
//! This one declares which complete policies a host may be armed with, and it
//! exists because of what arming a host looked like without it: a shell line
//! carrying eleven flags, in which the watermarks, the per-pass budget, the
//! unit label, the recovery program, the process names and the authorization
//! for ending a graphical session were all typed at the moment of the
//! incident, reviewed by nobody, and recorded only in one machine's registry.
//!
//! charless-mac-mini is what that costs. It carried a `report`-mode
//! declaration whose single repair was `restart_unit`, and the unit it named
//! kept a live process the whole time its listener was dying, so on
//! 2026-09-10 at 08:05:43Z the pass read 799 MiB available against a 2048 MiB
//! low watermark, recorded `pressure_active`, examined its one repair,
//! skipped it as `unit_running`, and reclaimed nothing. A policy that cannot
//! act on the pressure it reports is the shape
//! `stado.wisent.com/docs/checks-that-measure-nothing` collects, and the fix
//! is not a better shell line: it is a declaration whose repairs are chosen
//! for the host CLASS, kept beside the code that executes them, and read back
//! the same way on every host.
//!
//! Nothing here is applied by existing: a policy becomes a host's policy only
//! when a writer names it, `stado space watermark TARGET --policy NAME` is
//! that writer, and the graphical-session authorization inside a policy still
//! needs its own flag at that call.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use super::schema::{MemoryReclaimPolicy, REPAIR_GRAPHICAL_SESSION};

/// The one document declaring every policy a host may be armed with.
pub const DECLARATION_PATH: &str = "stado-rs/data/memory/policies.json";
const DECLARATION: &str = include_str!("../../../../../data/memory/policies.json");
const DECLARATION_SCHEMA_VERSION: u64 = 1;

/// The shortest summary that tells an operator what a policy does.
const MIN_SUMMARY_WORDS: usize = 12;

/// The release platforms a policy may be written for, spelled the way
/// `targets[].release_platform` spells them.
const RELEASE_PLATFORMS: [&str; 2] = ["darwin-arm64", "linux-amd64"];

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyDeclaration {
    schema_version: u64,
    memory_policies: Vec<DeclaredPolicy>,
}

/// One named, complete `memory_reclaim` declaration.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredPolicy {
    /// How a writer names this policy.
    pub name: String,
    /// What the policy does and why it is shaped that way, in the operator's
    /// own words rather than a restatement of its fields.
    pub summary: String,
    /// The `release_platform` values this policy is written for. A host of
    /// another platform is refused rather than silently armed with a unit
    /// label its service manager has never heard of.
    pub platforms: Vec<String>,
    /// The `role` values this policy is written for.
    pub roles: Vec<String>,
    /// The declaration itself, exactly as it lands in the registry.
    pub policy: MemoryReclaimPolicy,
}

impl DeclaredPolicy {
    /// Whether this policy authorizes ending processes of a logged-in
    /// graphical session.
    pub fn ends_graphical_session(&self) -> bool {
        self.policy
            .repair(REPAIR_GRAPHICAL_SESSION)
            .is_some_and(|repair| repair.allow_graphical_session)
    }

    /// The processes this policy would end, in declaration order.
    pub fn session_processes(&self) -> &[String] {
        self.policy
            .repair(REPAIR_GRAPHICAL_SESSION)
            .map_or(&[], |repair| repair.processes.as_slice())
    }

    /// Whether this policy is written for a host with this platform and role.
    /// An undeclared role matches nothing: a host nobody has classified is
    /// not quietly treated as a workstation.
    pub fn fits(&self, release_platform: &str, role: Option<&str>) -> bool {
        self.platforms
            .iter()
            .any(|declared| declared == release_platform)
            && role.is_some_and(|role| self.roles.iter().any(|declared| declared == role))
    }
}

static DECLARED_POLICIES: LazyLock<Result<Vec<DeclaredPolicy>, String>> =
    LazyLock::new(|| parse_declaration(DECLARATION));

fn parse_declaration(text: &str) -> Result<Vec<DeclaredPolicy>, String> {
    let declaration: PolicyDeclaration = serde_json::from_str(text)
        .map_err(|error| format!("{DECLARATION_PATH} is not a valid declaration: {error}"))?;
    if declaration.schema_version != DECLARATION_SCHEMA_VERSION {
        return Err(format!(
            "{DECLARATION_PATH} declares schema_version {}, and this build reads \
             {DECLARATION_SCHEMA_VERSION}",
            declaration.schema_version
        ));
    }
    if declaration.memory_policies.is_empty() {
        return Err(format!(
            "{DECLARATION_PATH} declares no policies; add at least one to memory_policies"
        ));
    }
    let mut names = BTreeSet::new();
    for declared in &declaration.memory_policies {
        let named = crate::deploy::py_str_repr(&declared.name);
        if declared.name.trim().is_empty() {
            return Err(format!("{DECLARATION_PATH} declares a policy with no name"));
        }
        if !names.insert(declared.name.clone()) {
            return Err(format!(
                "{DECLARATION_PATH} declares policy {named} more than once"
            ));
        }
        if declared.summary.split_whitespace().count() < MIN_SUMMARY_WORDS {
            return Err(format!(
                "{DECLARATION_PATH} policy {named} carries no usable summary; an operator picks \
                 a policy by what it does, so say it in at least {MIN_SUMMARY_WORDS} words"
            ));
        }
        if declared.platforms.is_empty() {
            return Err(format!(
                "{DECLARATION_PATH} policy {named} names no platforms; a policy no host's \
                 platform matches can never be applied"
            ));
        }
        for platform in &declared.platforms {
            if !RELEASE_PLATFORMS.contains(&platform.as_str()) {
                return Err(format!(
                    "{DECLARATION_PATH} policy {named} names platform {}, and this build knows {}",
                    crate::deploy::py_str_repr(platform),
                    RELEASE_PLATFORMS.join(", ")
                ));
            }
        }
        if declared.roles.is_empty() {
            return Err(format!(
                "{DECLARATION_PATH} policy {named} names no roles; a policy no host's role \
                 matches can never be applied"
            ));
        }
        // The same validator every registry writer goes through, so a policy
        // that could not be written to a target is a load error here rather
        // than a refusal an operator meets at the moment they need it.
        let document = serde_json::to_value(&declared.policy).map_err(|error| {
            format!("{DECLARATION_PATH} policy {named} cannot be serialized: {error}")
        })?;
        super::validate::validate(&document, &format!("{DECLARATION_PATH}:{}", declared.name))
            .map_err(|problem| {
                format!(
                    "{DECLARATION_PATH} policy {named} is not a coherent declaration: {} {}",
                    problem.location, problem.message
                )
            })?;
    }
    Ok(declaration.memory_policies)
}

/// Every policy a writer may name, in the document's order.
pub fn all() -> Result<&'static [DeclaredPolicy], String> {
    match &*DECLARED_POLICIES {
        Ok(policies) => Ok(policies),
        Err(error) => Err(error.clone()),
    }
}

/// The declared names, for a refusal that has to list them.
pub fn declared_names() -> Vec<&'static str> {
    all().map_or_else(
        |_| Vec::new(),
        |policies| policies.iter().map(|policy| policy.name.as_str()).collect(),
    )
}

/// One policy by name, refusing with the list of names that exist.
pub fn find(name: &str) -> Result<&'static DeclaredPolicy, String> {
    let policies = all()?;
    policies
        .iter()
        .find(|policy| policy.name == name)
        .ok_or_else(|| {
            format!(
                "{DECLARATION_PATH} declares no memory policy {}; it declares {}",
                crate::deploy::py_str_repr(name),
                declared_names().join(", ")
            )
        })
}

/// Every declared policy written for this platform and role.
pub fn fitting(release_platform: &str, role: Option<&str>) -> Vec<&'static DeclaredPolicy> {
    all().map_or_else(
        |_| Vec::new(),
        |policies| {
            policies
                .iter()
                .filter(|policy| policy.fits(release_platform, role))
                .collect()
        },
    )
}
