//! What one pass will act on, resolved against the registry: the managed
//! units, and the stable serving ports a legacy daemon can hold.

use serde_json::Value;

use super::MANAGED_AGENTS;
use crate::targets::ComputeTarget;

/// One managed unit this pass will act on, resolved against the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentPlan {
    /// launchd label.
    pub label: String,
    /// The unit file, as the target declares it or as
    /// [`MANAGED_AGENTS`] spells it for a host that declares nothing.
    pub plist: String,
    /// True when [`plist`](Self::plist) puts the unit in launchd's system
    /// domain, which this pass cannot bootstrap.
    pub privileged: bool,
}

/// Resolve every managed unit's plist against the target's own declaration.
///
/// `service::declared_services` already holds this fleet's rule — a
/// registry-declared record wins over the fixed list, because an operator
/// who adopted a recovery label said what its path is — so it is called
/// rather than re-implemented. Two opinions about where one unit lives is
/// what produced the wrong `missing_plist` in the first place.
pub fn plan_agents(target: &ComputeTarget) -> Vec<AgentPlan> {
    let declared = crate::deploy::service::declared_services(target);
    MANAGED_AGENTS
        .iter()
        .map(|(label, declared_elsewhere)| {
            let plist = declared
                .iter()
                .find(|service| service.matches(label))
                .map(|service| service.path.as_str())
                .filter(|path| !path.is_empty())
                .unwrap_or(declared_elsewhere);
            AgentPlan {
                label: (*label).to_string(),
                plist: plist.to_string(),
                privileged: crate::deploy::service::UnitDomain::from_path(plist)
                    .requires_privileged_bootstrap(),
            }
        })
        .collect()
}

/// One product's stable serving port on this host, and the legacy launchd
/// daemon that can hold it when nothing else does.
///
/// `release_control.products.<product>.targets.<host>` declares both:
/// `stable_bind` is the port every consumer's configuration names, and
/// `legacy_launchd_plist` is the unit that served it before the release
/// agent's blue-green proxy took it over. The pair exists for exactly that
/// handoff — `release_agent::stop_legacy` boots the label out on the way in
/// and `restore_legacy` bootstraps it back on the way out — and this is the
/// operator side of the same handoff, for the case where the agent cannot
/// come in at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StableBindPlan {
    /// The product the port belongs to, for the report.
    pub product: String,
    /// `host:port`, as the registry declares it.
    pub bind: String,
    /// The system LaunchDaemon that binds it directly.
    pub plist: String,
    /// That daemon's label, for `launchctl enable`.
    pub label: String,
    /// The blue-green candidate ports this product may be serving on instead.
    /// A live one means the release agent owns the handoff and this pass must
    /// keep its hands off the label.
    pub candidate_ports: Vec<u16>,
}

/// Every stable bind this host's `release_control` declares together with a
/// legacy daemon that can hold it.
///
/// A product that declares no `stable_bind` (a `replace` target) and one whose
/// target names no `legacy_launchd_plist` are both skipped: there is nothing
/// this pass could bootstrap for them, and a port it has no way to restore
/// reported beside the one it can is noise in front of the answer.
pub fn plan_stable_binds(document: &Value, target: &ComputeTarget) -> Vec<StableBindPlan> {
    let Ok(Some(control)) = crate::release_control::control(document) else {
        return Vec::new();
    };
    let mut plans: Vec<StableBindPlan> = control
        .products
        .iter()
        .filter_map(|(product, policy)| {
            let policy_target = policy.targets.get(&target.name)?;
            let bind = policy_target.stable_bind.as_deref()?;
            let plist = policy_target.legacy_launchd_plist.as_deref()?;
            let label = policy_target.legacy_launchd_label.as_deref()?;
            Some(StableBindPlan {
                product: product.clone(),
                bind: bind.to_string(),
                plist: plist.to_string(),
                label: label.to_string(),
                candidate_ports: policy_target
                    .candidate_ports
                    .map(|ports| ports.to_vec())
                    .unwrap_or_default(),
            })
        })
        .collect();
    plans.sort_by(|left, right| left.product.cmp(&right.product));
    plans
}
