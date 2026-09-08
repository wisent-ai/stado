//! Whether one platform ships the product's declared runtime.

use std::collections::BTreeSet;

use crate::release_pipeline::contract::recipe::{PlatformRecipe, RuntimeContract};

/// What one platform is, with respect to the product's `runtime` contract.
///
/// The contract is declared once for the product, but a product can now have
/// platforms that do not ship it: `jeden` is a Rust CLI with a documentation
/// site, and `stado web build` stages a site tarball with no binary in it.
/// Holding that platform to a contract about a binary refused a manifest that
/// was correct, so the contract applies to the platforms it describes.
///
/// The rule reads off the stage map, which is the only place a platform says
/// what it produces, and it is deliberately the least surprising one: both
/// destinations present means this platform ships the runtime, neither means
/// it ships something else, and one without the other is the half-staged
/// mistake the check was written to catch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeRole {
    /// Stages the runtime's binary and its launcher.
    Runtime,
    /// Stages neither, so the runtime contract says nothing about it.
    NotRuntime,
    /// Stages one of the two. A rollout of this would install something the
    /// host cannot start.
    HalfStaged,
}

/// One platform's role, from its staged destinations.
///
/// A product with no `runtime` has no runtime platforms, which is why the
/// contract is taken by reference rather than assumed: the same function then
/// answers for every manifest, and `publish` and the validator cannot drift
/// into two different readings of one rule.
pub fn runtime_role(
    destinations: &BTreeSet<&str>,
    runtime: Option<&RuntimeContract>,
) -> RuntimeRole {
    let Some(runtime) = runtime else {
        return RuntimeRole::NotRuntime;
    };
    match (
        destinations.contains(runtime.binary.as_str()),
        destinations.contains(runtime.launcher.as_str()),
    ) {
        (true, true) => RuntimeRole::Runtime,
        (false, false) => RuntimeRole::NotRuntime,
        _ => RuntimeRole::HalfStaged,
    }
}

/// The role of one platform of a parsed manifest, for callers that hold the
/// recipe rather than a set of destinations — the release pipeline's publish
/// step, which must not stamp a runtime coordinate onto a platform that ships
/// no runtime.
pub fn platform_runtime_role(
    recipe: &PlatformRecipe,
    runtime: Option<&RuntimeContract>,
) -> RuntimeRole {
    let destinations: BTreeSet<&str> = recipe.stage.values().map(String::as_str).collect();
    runtime_role(&destinations, runtime)
}
