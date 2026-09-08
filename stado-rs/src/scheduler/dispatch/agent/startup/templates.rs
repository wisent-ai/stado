//! Per-provider agent startup-script template selection.
//!
//! The template-registry half of the agent dispatcher: which baked-in
//! script text a provider's VMs boot into.

/// Per-provider agent startup-script templates, baked into the binary at
/// compile time exactly like the bundled compute-target registry
/// ([`crate::targets::load_bundled_registry`]). `data/templates/` stays
/// the single source of truth for the text; reading them back through
/// `crate::data_dir()` only ever worked on the build machine, because
/// that path is `CARGO_MANIFEST_DIR` frozen at compile time and an
/// installed `~/.stado/bin/stado` has no `data/` directory beside it.
///
/// Each launches `stado agent --kind <provider> --gpu-type <accel>
/// --idle-shutdown` after verifying and extracting the deployment-selected
/// immutable Python/model runtime bundle. Every provider receives the same
/// exact Stado release coordinates, storage/backup exports, and scoped
/// workload-secret identity.
///
/// Crate-visible so `crate::doctor` renders the preflight through the
/// identical template the dispatcher ships. A preflight that rendered its
/// own copy would prove nothing: the failure it exists to catch is a
/// placeholder no producer fills in THIS text.
pub(crate) fn bundled_template_for(provider_name: &str) -> Option<&'static str> {
    let variant =
        crate::capabilities::variant(crate::capabilities::RuntimeFacet::Execution, provider_name)?;
    match variant.adapter {
        crate::capabilities::RuntimeAdapter::Execution(
            crate::capabilities::ExecutionAdapter::Azure,
        ) => Some(include_str!(
            "../../../../../data/templates/startup_gpu_agent_azure.sh"
        )),
        crate::capabilities::RuntimeAdapter::Execution(
            crate::capabilities::ExecutionAdapter::Aws,
        ) => Some(include_str!(
            "../../../../../data/templates/startup_gpu_agent_aws.sh"
        )),
        crate::capabilities::RuntimeAdapter::Execution(
            crate::capabilities::ExecutionAdapter::Gcp
            | crate::capabilities::ExecutionAdapter::Box
            | crate::capabilities::ExecutionAdapter::Local
            | crate::capabilities::ExecutionAdapter::Vast,
        ) => Some(include_str!(
            "../../../../../data/templates/startup_gpu_agent.sh"
        )),
        _ => None,
    }
}
