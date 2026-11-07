//! Every other reason the janitor keeps a checkout a session is proven to
//! have cloned, each at the rule on the work root's session checkouts: a
//! stash, a repository with no remote, another live worktree of it, an
//! untracked file the checkout's own configuration hides from `git status`
//! (all `unpushed_work`, named among the pass's errors), git that cannot
//! answer (`git_state_unread`) and a launchd unit naming a path in it
//! (`named_by_unit`). None is taken.

use std::fs;

use stado::providers::local::work_base::{SESSION_CHECKOUTS_LEAF, SESSION_CLONE_MARKER};

use super::{cleanup_with_work_root, errors_naming, git, origin, session_clone, CLEANER};
use crate::native::Native;

#[test]
fn a_stash_no_remote_another_worktree_unreadable_git_or_a_unit_keeps_a_proven_checkout() {
    let native = Native::new("session-checkouts-keeps");
    let origin = origin(&native);
    let work_root = native.home.join("fleet-work");
    let sessions = work_root.join(SESSION_CHECKOUTS_LEAF);

    let stashed = sessions.join("stashed-repository");
    session_clone(&origin, &stashed);
    fs::write(stashed.join("README"), b"a change put aside\n").unwrap();
    git(&stashed, &["stash", "--quiet"]);

    let unremoted = sessions.join("unremoted-repository");
    fs::create_dir_all(&unremoted).unwrap();
    git(&unremoted, &["init", "--quiet"]);
    fs::write(unremoted.join("README"), b"history only here\n").unwrap();
    git(&unremoted, &["add", "README"]);
    git(&unremoted, &["commit", "--quiet", "-m", "only here"]);
    let marker = unremoted.join(".git").join(SESSION_CLONE_MARKER);
    fs::write(marker, b"session-checkouts-journey-job\n").unwrap();

    let branched = sessions.join("branched-repository");
    session_clone(&origin, &branched);
    let side = native.home.join("fixtures/side-worktree");
    git(&branched, &["worktree", "add", "--quiet", "-b", "side", side.to_str().unwrap()]);

    let broken = sessions.join("broken-repository");
    session_clone(&origin, &broken);
    fs::write(broken.join(".git/HEAD"), b"no reference git can read\n").unwrap();

    let served = sessions.join("served-repository");
    session_clone(&origin, &served);
    let agents = native.home.join("Library/LaunchAgents");
    fs::create_dir_all(&agents).unwrap();
    let unit = format!(
        "<plist><dict><key>WorkingDirectory</key><string>{}</string></dict></plist>\n",
        served.display()
    );
    fs::write(agents.join("com.example.session-served.plist"), unit).unwrap();

    // The checkout's own configuration hides untracked files from `git
    // status`; the new file is the session's work all the same.
    let hidden = sessions.join("hidden-untracked-repository");
    session_clone(&origin, &hidden);
    git(&hidden, &["config", "status.showUntrackedFiles", "no"]);
    fs::write(hidden.join("UNSEEN"), b"never committed\n").unwrap();

    native.fill_with_user_data();
    let report = cleanup_with_work_root(&native, &work_root);
    let cleaner = &report["cleaners"][CLEANER];
    assert_eq!(report["rule"]["triggered"], true, "{report}");
    for kept in [&stashed, &unremoted, &branched, &broken, &served, &hidden] {
        assert!(kept.join(".git").is_dir(), "{} was taken: {report}", kept.display());
    }
    assert_eq!(cleaner["deleted_items"], 0, "{report}");
    assert_eq!(cleaner["skipped"]["unpushed_work"], 4, "{report}");
    assert_eq!(cleaner["skipped"]["git_state_unread"], 1, "{report}");
    assert_eq!(cleaner["skipped"]["named_by_unit"], 1, "{report}");
    let named = [
        "stashes",
        "no remote",
        "other worktrees",
        "broken-repository",
        "UNSEEN",
    ];
    for named in named {
        assert!(errors_naming(&report, named), "{named} is not named: {report}");
    }
}
