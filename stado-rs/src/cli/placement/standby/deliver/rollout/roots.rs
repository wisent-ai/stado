//! How a release-control product's install root reads for a standby host:
//! made relative to `{home}`, and whether it is the directory its catalog
//! program runs from.

use crate::release_control::HOME_PLACEHOLDER;

/// The product's install root written for every host: an absolute root under
/// the placed host's home becomes the same path under `{home}`, which the
/// release agent expands to each target's own home, so the placed host keeps
/// its root and the new host gets one under its home instead of the placed
/// host's. `None` when the root already names `{home}` or lies elsewhere.
pub(super) fn home_relative(install_root: &str, placed_home: &str) -> Option<String> {
    let rest = install_root.strip_prefix(placed_home)?;
    (rest.starts_with('/') && !install_root.contains(HOME_PLACEHOLDER))
        .then(|| format!("{HOME_PLACEHOLDER}{rest}"))
}

/// Whether a release-control install root is the directory the catalog
/// program of `tree` runs from, `<home>/.stado/services/<tree>`.
pub(super) fn installs_at_tree(install_root: &str, tree: &str) -> bool {
    let directory = format!("/{}/{tree}", crate::deploy::artifact_install::SERVICES_ROOT);
    install_root.trim_end_matches('/').ends_with(&directory)
}
