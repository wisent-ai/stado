//! Who built this artifact, and can anyone still find the source it came from?
//!
//! An install can report a version no commit on any branch of this
//! repository has ever carried, which means the binary running the fleet's
//! control plane came out of a source tree nobody else can produce. Nothing
//! on the host and nothing in this repository can say otherwise: the install
//! recorded a name, a size and a timestamp, none of which is a producer. A
//! worker release built on a laptop and never published is the same story
//! with a different artifact.
//!
//! Both happened because installing is one command and releasing is a
//! pipeline, and nothing ever asked the cheaper path for its receipts. This
//! module is that question, asked at the only moment when the answer is still
//! available: on the machine doing the building, where the checkout the
//! artifact came out of is still on disk.
//!
//! Two facts are recorded and they are not the same fact:
//!
//!   sha256  what was installed. Identifies the bytes, and identifies nothing
//!           about where they came from -- a hash of an untraceable build is
//!           an untraceable build with a name.
//!   commit  who produced them. `unprovenanced` when the artifact did not come
//!           out of a checkout's `target/` directory at all, which is the
//!           honest answer and the one 0.7.1 would have given.
//!
//! Reachability is deliberately separate from both, and is not stored: whether
//! a commit is an ancestor of `origin/main` is true or false depending on when
//! it is asked, because a commit can be pushed the hour after an install and a
//! branch can be force-pushed out from under one. Freezing that answer into
//! the manifest would preserve a verdict past the point where it is about
//! anything. The manifest keeps the commit; the reader resolves the rest
//! against a checkout it can see.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

/// What the commit field says when there is no commit to name.
///
/// A literal rather than an absent field, because an absent field reads as "we
/// have not got to it yet" and this is a settled finding: there is no producer
/// for these bytes. Readers compare against this exact string.
pub const UNPROVENANCED: &str = "unprovenanced";

/// The branch that decides whether a commit is real for this fleet.
///
/// Not `HEAD`, and not "any branch": a build from a local branch that was
/// never pushed is exactly the 0.7.1 case, and it would pass a check that
/// merely asked whether the object exists in some clone somewhere.
const PUBLISHED_BRANCH: &str = "origin/main";

/// Length of a full git object id, written out because a short id is ambiguous
/// over the lifetime of a repository and this record outlives the terminal it
/// was printed in.
const COMMIT_ID_LENGTH: usize = 40;

/// The manifest delivered beside an artifact, at
/// `~/.stado/provenance/<artifact>.json` on the host that carries it.
///
/// Stored on the target rather than centrally on purpose. A central ledger
/// records what an install command believed it did; a file next to the binary
/// is a property of the machine, and survives the ledger being lost, rebuilt,
/// or simply never consulted. It is the host that gets asked "what are you
/// running", so it is the host that has to be able to answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    /// Basename the artifact was installed under, e.g. `stado`.
    pub artifact: String,
    /// Digest of the exact bytes delivered, lowercase hex.
    pub sha256: String,
    /// Source commit, 40 lowercase hex digits, or [`UNPROVENANCED`].
    pub commit: String,
    /// Hostname of the machine that built and delivered it.
    pub builder: String,
    /// RFC 3339, UTC, when the manifest was made.
    pub at: String,
}

impl Provenance {
    /// Does this record name a commit at all?
    ///
    /// Distinct from reachability: a named commit nobody can find is a
    /// different operator problem from an artifact that never had one.
    pub fn names_a_commit(&self) -> bool {
        is_commit_id(&self.commit)
    }
}

/// Is this string a full git object id, rather than a sentinel or a fragment?
pub fn is_commit_id(value: &str) -> bool {
    value.len() == COMMIT_ID_LENGTH && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Is `commit` an ancestor of `origin/main` in `repo`?
///
/// `merge-base --is-ancestor` answers by exit status: zero for yes, one for
/// no, and something else for "that object is not in this repository", which
/// is also a no as far as an operator is concerned -- an artifact whose commit
/// this clone has never heard of is exactly as untraceable as one with no
/// commit at all.
pub fn reachable_in_repo(commit: &str, repo: &Path) -> bool {
    is_commit_id(commit)
        && crate::wait::output(Command::new("git").arg("-C").arg(repo).args([
            "merge-base",
            "--is-ancestor",
            commit,
            PUBLISHED_BRANCH,
        ]))
        .is_ok_and(|output| output.status.success())
}

/// The checkout an artifact was built in: the tree enclosing the `target/`
/// directory it sits under.
///
/// The build root beside Cargo's `target/` can be a nested crate rather
/// than the repository root. A `.git` entry can also be a file. Ask Git
/// for the checkout boundary instead of inferring it from directory shape;
/// Git refuses when the build root is outside a repository.
///
/// Walked from the artifact outwards, so a nested or vendored checkout
/// resolves to the tree that actually built it rather than to whatever
/// encloses that tree.
pub fn source_repo(path: &Path) -> Option<PathBuf> {
    let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    absolute
        .ancestors()
        .filter(|ancestor| ancestor.file_name() == Some(std::ffi::OsStr::new("target")))
        .filter_map(Path::parent)
        .find_map(checkout_root)
}

/// The root of the checkout `directory` belongs to, or `None` when it belongs
/// to none.
///
/// `release newest` reads it to find the workspace: one checkout per
/// repository side by side means the workspace is that root's parent.
pub fn checkout_root(directory: &Path) -> Option<PathBuf> {
    git(directory, &["rev-parse", "--show-toplevel"]).map(PathBuf::from)
}

/// A checkout this process can ask reachability questions of.
///
/// Read-back runs on an operator's machine, long after and possibly far from
/// the build, so the repository has to be found rather than remembered. The
/// running binary's own checkout comes first, because
/// `./target/release/stado host provenance ...` during an incident already
/// names the right tree; the working directory is the fallback for an
/// installed CLI invoked from inside the source. Neither existing means
/// reachability is unknown, and unknown is then reported as unknown rather
/// than folded into "not reachable".
pub fn local_repo() -> Option<PathBuf> {
    if let Some(repo) = std::env::current_exe()
        .ok()
        .and_then(|exe| source_repo(&exe))
    {
        return Some(repo);
    }
    checkout_root(&std::env::current_dir().ok()?)
}

/// One `git` read against a checkout, trimmed, or `None` if it did not run or
/// did not succeed. Local reads only: this never touches a host and never
/// fetches, so an offline builder still gets a truthful `HEAD`.
fn git(repo: &Path, arguments: &[&str]) -> Option<String> {
    let output =
        crate::wait::output(Command::new("git").arg("-C").arg(repo).args(arguments)).ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}
