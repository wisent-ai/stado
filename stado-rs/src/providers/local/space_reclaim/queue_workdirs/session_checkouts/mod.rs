//! The git checkouts fleet sessions cloned on a host, taken by the janitor's
//! own pass once the session that cloned one is proven, nothing in it exists
//! only there and no process works in it: below the home on every pass,
//! since nothing reuses a checkout there any more, below the work root at the
//! disk-full rule, since the next session reuses it.
//!
//! A detached coding session clones the repository it works on into a
//! checkout on the host it runs on, and nothing ever removed one. Earlier
//! releases cloned it under the agent's home, at
//! `~/Documents/CodingProjects/<repository>` and then at
//! `~/Documents/CodingProjects/Wisent/<repository>`: on a Linux fleet host
//! those checkouts filled the home volume whose 80% rule stops the host from
//! claiming work, and every reclaim counted them as the user's. A session now
//! clones under the host's declared work root, at `<work root>/
//! session-checkouts/<repository>` ([`work_base::SESSION_CHECKOUTS_LEAF`]),
//! and writes the clone marker into the checkout's `.git`.
//!
//! A candidate is a directory holding `.git` directly below one of those
//! roots, or, below the home's root, one level further down. The same
//! folders hold a person's own checkouts, so the place proves nothing: a
//! checkout is taken only on the proof that a fleet session cloned it
//! ([`evidence`]: its clone marker, or this host's queue record of the
//! session that cloned it), and every other one is kept as
//! `not_session_checkout`. Below the home a checkout is never taken on a
//! shared host (`shared_host_not_stado_owned`), and on macOS the folder is
//! opened only when the host's entry declares the Documents grant for this
//! program. This host's queue is asked only when a checkout below the home
//! carries no clone marker.
//!
//! A proven checkout is kept, the reason counted, when a registry service
//! declaration or a launchd or systemd unit names a path in it, and when git
//! reports anything in it that no remote has (`unpushed_work`, named with
//! what it is among the pass's errors; git is asked with the settings that
//! would hide an untracked file or a changed submodule overridden) or cannot
//! answer (`git_state_unread`).
//! Otherwise it is taken whole under the janitor's exclusive cleanup lock for
//! that one checkout, after reading what live processes hold right then
//! (`held_by_process`): it is moved aside in one rename and then removed, so
//! a removal that fails half way leaves a name no tool reads, which the next
//! pass removes. Each removal is a line of the janitor's removal record.

mod evidence;
mod take;
mod work;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::providers::local::space_reclaim::build_caches::privacy_protected_parts;
use crate::providers::local::space_reclaim::janitor::policy::ownership;
use crate::providers::local::space_reclaim::janitor::state::report::{met::Met, CleanupReport};
use crate::providers::local::space_reclaim::weles::aside_origin;
use crate::providers::local::space_reclaim::JanitorError;
use crate::providers::local::work_base;
use crate::targets::ComputeTarget;

/// The name this cleaner's report is filed under.
pub const CLEANER: &str = "session_checkouts";

/// Home-relative directory earlier releases cloned session checkouts into:
/// `<repository>` directly below it, and later `Wisent/<repository>`.
pub const HOME_ROOT: &str = "Documents/CodingProjects";

/// What one pass hands this cleaner.
pub(in crate::providers::local::space_reclaim) struct Pass<'a> {
    pub home: &'a Path,
    /// Whether checkouts are removed or only judged and counted.
    pub enforcing: bool,
    /// The one tree below the home a pass may delete in, `None` for the
    /// whole host ([`ownership::deletable_root`]).
    pub deletable: Option<&'a Path>,
    /// This host's registry entry, when it was read: its service
    /// declarations name the paths no cleaner takes, and its hostnames say
    /// which queue records are this host's.
    pub target: Option<&'a ComputeTarget>,
    /// The consent-gated folders this host's entry lets this program open.
    pub walked_folders: &'a BTreeSet<String>,
    /// The janitor's state directory: its lock and its removal record.
    pub lock_dir: &'a Path,
    pub writer: &'a str,
    /// Whether the pass is at the disk-full rule: only then are the work
    /// root's checkouts, which the next session reuses, taken.
    pub at_rule: bool,
}

/// Where a checkout lies.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    /// Below the home: a person's checkouts lie there too.
    Home,
    /// Below the declared work root's session checkouts directory.
    WorkRoot,
}

/// One root checkouts are listed in.
struct Root {
    path: PathBuf,
    /// Whether a directory holding no `.git` is looked into one level down.
    nested: bool,
    place: Place,
}

/// The home's root, and the work root's when the host declares one and the
/// pass is at the rule.
fn roots(home: &Path, at_rule: bool) -> Vec<Root> {
    let mut roots = vec![Root {
        path: home.join(HOME_ROOT),
        nested: true,
        place: Place::Home,
    }];
    if let Some(work_root) = work_base::declared().filter(|_| at_rule) {
        roots.push(Root {
            path: work_root.join(work_base::SESSION_CHECKOUTS_LEAF),
            nested: false,
            place: Place::WorkRoot,
        });
    }
    roots
}

/// Whether the home's root lies in a folder no walk of this home opens.
fn home_root_protected(walked_folders: &BTreeSet<String>) -> bool {
    let folder = HOME_ROOT.split('/').next().unwrap_or(HOME_ROOT);
    privacy_protected_parts(cfg!(target_os = "macos"), walked_folders)
        .iter()
        .any(|part| *part == folder)
}

/// What the roots hold: the checkouts, and the checkouts an earlier pass
/// moved aside and could not finish removing.
#[derive(Default)]
struct Found {
    checkouts: Vec<(PathBuf, Place)>,
    aside: Vec<PathBuf>,
}

impl Found {
    /// File `path` as a tree moved aside or as a checkout; whether it was
    /// either.
    fn file(&mut self, path: &Path, place: Place) -> bool {
        if aside_origin(path).is_some() {
            self.aside.push(path.to_path_buf());
            return true;
        }
        if work::is_checkout(path) {
            self.checkouts.push((path.to_path_buf(), place));
            return true;
        }
        false
    }
}

/// List every root this host has.
fn list(pass: &Pass<'_>, met: &mut Met, report: &mut CleanupReport) -> Found {
    let mut found = Found::default();
    for root in roots(pass.home, pass.at_rule) {
        if root.place == Place::Home && home_root_protected(pass.walked_folders) {
            met.skipped("privacy_protected");
            continue;
        }
        // On a shared host the home's checkouts are its person's: none is
        // listed or sized, now that every pass looks at the home's root.
        if root.place == Place::Home && !ownership::may_delete(pass.deletable, &root.path) {
            met.skipped(ownership::NOT_STADO_OWNED);
            continue;
        }
        let children = match work::directories(&root.path) {
            Ok(children) => children,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                met.skipped("root_absent");
                continue;
            }
            Err(error) => {
                report.add_error(
                    CLEANER,
                    &JanitorError::os(&format!(
                        "the session checkouts under {} cannot be listed: {error}",
                        root.path.display()
                    )),
                );
                continue;
            }
        };
        for child in children {
            if found.file(&child, root.place) || !root.nested {
                continue;
            }
            match work::directories(&child) {
                Ok(nested) => {
                    for path in nested {
                        found.file(&path, root.place);
                    }
                }
                Err(error) => report.add_error(
                    CLEANER,
                    &JanitorError::os(&format!(
                        "{} cannot be listed for session checkouts: {error}",
                        child.display()
                    )),
                ),
            }
        }
    }
    found
}

/// The sessions this host's queue records, asked only when a checkout below
/// the home this pass may delete in carries no clone marker: that proof is
/// the only one such a checkout can have. `None` when nothing needs them or
/// they could not be read, the cause then among the pass's errors and every
/// such checkout kept as `not_session_checkout`.
async fn session_jobs(
    pass: &Pass<'_>,
    found: &Found,
    report: &mut CleanupReport,
) -> Option<Vec<evidence::SessionJob>> {
    let unmarked = found.checkouts.iter().any(|(path, place)| {
        *place == Place::Home
            && ownership::may_delete(pass.deletable, path)
            && evidence::marker(path).is_none()
    });
    if !unmarked {
        return None;
    }
    let Some(target) = pass.target else {
        report.add_error(
            CLEANER,
            &JanitorError::os(&format!(
                "this host's registry entry was not read, so no queue record can prove which \
                 checkouts under ~/{HOME_ROOT} a session cloned; each is kept as \
                 not_session_checkout until a pass reads the entry"
            )),
        );
        return None;
    };
    match evidence::ask(target).await {
        Ok(jobs) => Some(jobs),
        Err(cause) => {
            report.add_error(
                CLEANER,
                &JanitorError::os(&format!(
                    "the checkouts under ~/{HOME_ROOT} that carry no clone marker are kept as \
                     not_session_checkout: {cause}; once the queue answers, the next pass at \
                     the rule judges them again"
                )),
            );
            None
        }
    }
}

/// One pass over every session checkout: what an earlier pass moved aside
/// is removed, and each checkout is judged and, in an enforcing pass, taken
/// unless something keeps it.
pub(in crate::providers::local::space_reclaim) async fn scan_session_checkouts(
    pass: &Pass<'_>,
    report: &mut CleanupReport,
) {
    let mut met = Met::default();
    let found = list(pass, &mut met, report);
    met.listed(found.checkouts.len() + found.aside.len());
    for aside in &found.aside {
        take::remove_aside(pass, aside, &mut met, report);
    }
    let jobs = session_jobs(pass, &found, report).await;
    let declared = pass
        .target
        .map(crate::deploy::host_reclaim::declared_paths)
        .unwrap_or_default();
    let judged = take::Judged {
        declared: &declared,
        jobs: jobs.as_deref(),
    };
    for (checkout, place) in &found.checkouts {
        take::take(pass, &judged, checkout, *place, &mut met, report);
    }
    match met.report() {
        Ok(record) => report.session_checkouts = record,
        Err(cause) => report.add_error(CLEANER, &JanitorError::os(&cause)),
    }
}
