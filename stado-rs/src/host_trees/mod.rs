//! The host tree register: every tree a fleet writer creates, or once
//! created, on a host, with the directory it hangs from, the verbs that write
//! and read it, and what reclaims it or why nothing does.
//!
//! The paths of the fleet's trees were spelled by hand in each writer,
//! reader and cleaner, and nothing checked what a fleet verb actually wrote:
//! a writer could make a tree no cleaner owned, whose bytes were never taken
//! and which `stado space report` counted as the user's, and a reader could
//! name a tree its writer never made. The register is where such a tree is
//! declared. `stado space report --unregistered` ([`inventory`]) lists, on
//! every host, each tree that no entry holds, and the release gate
//! `host-trees-register` runs the writer verbs in an isolated home and work
//! root and fails on a new tree no entry holds.

mod entries;
pub(crate) mod inventory;

pub use entries::{
    entries, runner_roots, MANAGED_RUNS_STAGE, RUNNER_SCRATCH, RUN_AREA, SIGNAL_AREA,
};

/// The directory a tree hangs from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base {
    /// The host's work base: its declared work root, else `$HOME/.stado/work`
    /// ([`crate::providers::local::work_base::HOME_WORK_COMPONENTS`]).
    WorkBase,
    /// The host's declared work root only; a host that declares none has no
    /// such tree, since its writer refuses to start there.
    WorkRoot,
    /// The account's home.
    Home,
    /// Every runner root `data/work/runner-profiles.json` declares.
    Runner,
}

/// Why an entry is in the register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A tree a current fleet verb writes.
    Writer,
    /// A tree an earlier release wrote and no current verb writes.
    Legacy,
    /// The account's own tree, or a tool's the host was provisioned with.
    Account,
}

/// What takes an entry's tree back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reclaim {
    /// The janitor cleaner or `stado space reclaim` stage of this name.
    By(&'static str),
    /// Nothing, for this reason.
    Kept(&'static str),
}

/// One tree a host carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: &'static str,
    pub base: Base,
    /// The tree below its base, `/`-separated. A component may carry one
    /// `*`, which matches any text inside that component.
    pub path: String,
    pub kind: Kind,
    pub writers: &'static [&'static str],
    pub readers: &'static [&'static str],
    pub reclaim: Reclaim,
}

impl Entry {
    /// The entry's tree as a path below the directory it is listed in on a
    /// host: the work base is the declared work root when there is one and
    /// the home's `.stado/work` otherwise; a work-root entry is nowhere on a
    /// host that declares none.
    fn located(&self, declared: bool) -> Option<(Base, String)> {
        match (self.base, declared) {
            (Base::WorkBase, true) | (Base::WorkRoot, true) => {
                Some((Base::WorkRoot, self.path.to_string()))
            }
            (Base::WorkBase, false) => Some((
                Base::Home,
                format!(
                    "{}/{}",
                    crate::providers::local::work_base::HOME_WORK_COMPONENTS.join("/"),
                    self.path
                ),
            )),
            (Base::WorkRoot, false) => None,
            (base, _) => Some((base, self.path.to_string())),
        }
    }
}

/// The work tree of job `job_id` on `target`, as a shell word that expands
/// there: `<work root>/jobs/wc-<job>` on a host that declares a work root,
/// `$HOME/.stado/work/jobs/wc-<job>` otherwise, the tree the agent's job
/// start makes (`queue_workdirs::roots::create_work_dir`). Every reader of a
/// job's tree on another host names it through here.
pub fn job_tree_word(target: &crate::targets::ComputeTarget, job_id: &str) -> String {
    use crate::providers::local::{space_reclaim::queue_workdirs::WORKDIR_PREFIX, work_base};
    let leaf = crate::deploy::shlex_quote(&format!("{WORKDIR_PREFIX}{job_id}"));
    match target.work_root.as_deref() {
        Some(root) => format!(
            "{}/{}/{leaf}",
            crate::deploy::shlex_quote(root),
            work_base::JOBS_LEAF
        ),
        None => format!(
            "\"$HOME\"/{}/{leaf}",
            work_base::HOME_JOBS_COMPONENTS.join("/")
        ),
    }
}

/// Whether `pattern`, a component that may carry one `*`, names `name`.
fn component_matches(pattern: &str, name: &str) -> bool {
    match pattern.split_once('*') {
        Some((prefix, suffix)) => {
            name.len() >= prefix.len() + suffix.len()
                && name.starts_with(prefix)
                && name.ends_with(suffix)
        }
        None => pattern == name,
    }
}

/// The entry holding `child`, a directory listed below `listed` (a path
/// below `base`, empty for the base itself), on a host that declares a work
/// root or not: an entry whose tree is `listed/child`, lies below it, or
/// holds `listed` whole.
pub fn holder(base: Base, listed: &str, child: &str, declared: bool) -> Option<Entry> {
    let listed: Vec<&str> = listed.split('/').filter(|part| !part.is_empty()).collect();
    entries().into_iter().find(|entry| {
        let Some((entry_base, path)) = entry.located(declared) else {
            return false;
        };
        if entry_base != base {
            return false;
        }
        let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        let within = parts.len() <= listed.len()
            && parts
                .iter()
                .zip(&listed)
                .all(|(pattern, name)| component_matches(pattern, name));
        within
            || (parts.len() > listed.len()
                && parts
                    .iter()
                    .zip(listed.iter().chain(std::iter::once(&child)))
                    .all(|(pattern, name)| component_matches(pattern, name)))
    })
}
