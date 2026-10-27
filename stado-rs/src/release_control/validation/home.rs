//! Whether a product's install root is written for one host's home while the
//! product targets hosts whose homes differ. Each target installs at the root
//! with `{home}` expanded to its own home; a root spelled under one home
//! instead sends every other target to a directory that is not its own.

use crate::release_control::ProductReleasePolicy;

/// What the install root is written in place of for each target's home.
pub const HOME_PLACEHOLDER: &str = "{home}";

/// One target a home-bound install root sends outside its own home.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignTarget {
    /// The target installing where it is not at home.
    pub host: String,
    /// That target's home.
    pub home: String,
    /// The target whose home the root is written under.
    pub owner: String,
    /// The root as it should read: `{home}` and what follows the owner's home.
    pub portable_root: String,
}

/// Each target `policy`'s install root sends outside its own home: none when
/// the root carries `{home}`, or lies under no target's home, or every
/// target shares the home it lies under.
pub fn foreign_targets(policy: &ProductReleasePolicy) -> Vec<ForeignTarget> {
    let root = policy.install_root.as_str();
    if root.contains(HOME_PLACEHOLDER) {
        return Vec::new();
    }
    let owner = policy.targets.iter().find_map(|(owner, target)| {
        root.strip_prefix(target.home.as_str())
            .filter(|rest| rest.starts_with('/'))
            .map(|rest| (owner, target.home.as_str(), rest))
    });
    let Some((owner, owner_home, rest)) = owner else {
        return Vec::new();
    };
    policy
        .targets
        .iter()
        .filter(|(_, target)| target.home != owner_home)
        .map(|(host, target)| ForeignTarget {
            host: host.clone(),
            home: target.home.clone(),
            owner: owner.clone(),
            portable_root: format!("{HOME_PLACEHOLDER}{rest}"),
        })
        .collect()
}
