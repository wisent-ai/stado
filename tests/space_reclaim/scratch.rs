//! `stado space scratch register` makes a tree an agent exported the
//! janitor's to remove: at the disk-full rule a registered tree whose owner
//! process has ended goes, one whose owner still runs stays, an unregistered
//! tree beside them stays, and the removal is in the janitor's record. A
//! registration outside the home, inside Stado's own tree or `~/.ssh`, of a
//! link, through a link or `..`, or without its owner's process is refused
//! naming it. Run on a real APFS volume under the repository's build
//! directory.

use std::fs;
use std::os::unix::fs::symlink;
use std::process::{Child, Command};

use crate::native::Native;

/// A process that runs until the test kills it: a live owner session.
fn live_owner() -> Child {
    Command::new("/bin/sleep").arg("600").spawn().unwrap()
}

/// The id of a process that has ended: an owner session that is over.
fn ended_owner() -> i32 {
    let mut child = Command::new("/usr/bin/true").spawn().unwrap();
    let pid = i32::try_from(child.id()).unwrap();
    child.wait().unwrap();
    pid
}

#[test]
fn a_registered_tree_of_an_ended_session_goes_at_the_rule_and_the_rest_stays() {
    let native = Native::new("scratch");
    let ended = native.home.join("checkout/.build/agent-export-ended");
    let running = native.home.join("checkout/.build/agent-export-running");
    let unregistered = native.home.join("checkout/.build/never-registered");
    for tree in [&ended, &running, &unregistered] {
        fs::create_dir_all(tree.join("src")).unwrap();
        fs::write(tree.join("src/main.rs"), vec![b'x'; 64 * 1024]).unwrap();
    }
    let mut owner = live_owner();
    for (tree, pid, session) in [
        (&ended, ended_owner(), "agent-session-ended"),
        (
            &running,
            i32::try_from(owner.id()).unwrap(),
            "agent-session-live",
        ),
    ] {
        native.success(&[
            "space",
            "scratch",
            "register",
            tree.to_str().unwrap(),
            "--creator",
            "test agent",
            "--purpose",
            "an export of a commit for its build",
            "--owner-session",
            session,
            "--owner-pid",
            &pid.to_string(),
        ]);
    }
    let listed = native.json(&["space", "scratch", "list", "--json"]);
    native.observe("scratch list", listed.clone());
    assert_eq!(listed["entries"].as_array().unwrap().len(), 2, "{listed}");
    let ranked = native.json(&["space", "candidates", "--min-bytes", "1", "--json"]);
    native.observe("candidates", ranked.clone());
    let registered_rows: Vec<_> = ranked["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["cleaner"] == "registered_scratch")
        .collect();
    assert_eq!(registered_rows.len(), 2, "{ranked}");
    assert!(
        registered_rows
            .iter()
            .all(|row| row["class"] == "agent_output" && row["action"] == "automatic"),
        "{ranked}"
    );

    let user_data = native.fill_with_user_data();
    let report = native.cleanup();
    let cleaner = &report["cleaners"]["registered_scratch"];
    assert_eq!(cleaner["deleted_items"], 1, "{report}");
    assert_eq!(cleaner["skipped"]["owner_session_running"], 1, "{report}");
    assert!(
        !ended.exists(),
        "the ended session's tree was kept: {report}"
    );
    assert!(
        running.join("src/main.rs").is_file(),
        "a running session's tree was taken: {report}"
    );
    assert!(
        unregistered.join("src/main.rs").is_file(),
        "an unregistered tree was taken: {report}"
    );
    assert!(user_data.is_file(), "the user's data was deleted");
    owner.kill().unwrap();
    owner.wait().unwrap();

    let after = native.json(&["space", "scratch", "list", "--json"]);
    native.observe("scratch list after", after.clone());
    let entries = after["entries"].as_array().unwrap();
    assert_eq!(
        entries.len(),
        1,
        "the removed tree is still registered: {after}"
    );
    let record = native.json(&["space", "candidates", "--json"]);
    assert!(
        record["removed"].as_array().unwrap().iter().any(|line| {
            line["cleaner"] == "registered_scratch"
                && line["path"]
                    .as_str()
                    .is_some_and(|path| path.ends_with("agent-export-ended"))
        }),
        "the removal is not recorded: {record}"
    );
}

#[test]
fn a_registration_outside_the_home_inside_stado_or_of_a_link_is_refused() {
    let native = Native::new("scratch-refusals");
    let stado_tree = native.home.join(".stado/work/held");
    fs::create_dir_all(&stado_tree).unwrap();
    let ssh = native.home.join(".ssh");
    fs::create_dir_all(&ssh).unwrap();
    let target = native.home.join("real");
    fs::create_dir_all(target.join("sub")).unwrap();
    let link = native.home.join("link");
    symlink(&target, &link).unwrap();
    // A path that spells a tree inside the home but reaches `~/.ssh`, or the
    // home's parent, through `..`; and one that reaches a tree through a
    // link on its way.
    let up_to_ssh = format!("{}/real/../.ssh", native.home.display());
    let up_out = format!("{}/real/../..", native.home.display());
    let through_link = link.join("sub");
    let outside = native.home.parent().unwrap().to_path_buf();
    for (path, expected) in [
        (outside.to_str().unwrap(), "is not below the home"),
        (
            stado_tree.to_str().unwrap(),
            "which Stado's own cleaners manage",
        ),
        (ssh.to_str().unwrap(), "which Stado's own cleaners manage"),
        (link.to_str().unwrap(), "a link is refused"),
        (up_to_ssh.as_str(), "names `.` or `..`"),
        (up_out.as_str(), "names `.` or `..`"),
        (through_link.to_str().unwrap(), "through a link on the way"),
        (
            native.home.join("absent").to_str().unwrap(),
            "create the tree first",
        ),
    ] {
        let args = [
            "space",
            "scratch",
            "register",
            path,
            "--creator",
            "test",
            "--purpose",
            "refusal",
            "--owner-session",
            "s",
            "--owner-pid",
            "1",
        ];
        let output = native.run(&args);
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        assert!(
            !output.status.success() && stderr.contains(expected),
            "registering {path} was not refused with {expected:?}: {stderr}"
        );
    }
    // The owner's process is named, never guessed from the shell that ran
    // the command, which ends with it.
    let unowned = native.run(&[
        "space",
        "scratch",
        "register",
        target.to_str().unwrap(),
        "--creator",
        "test",
        "--purpose",
        "p",
        "--owner-session",
        "s",
    ]);
    let stderr = String::from_utf8_lossy(&unowned.stderr).to_string();
    assert!(
        !unowned.status.success() && stderr.contains("--owner-pid"),
        "a registration without its owner's process was taken: {stderr}"
    );
    let empty = native.run(&[
        "space",
        "scratch",
        "register",
        target.to_str().unwrap(),
        "--creator",
        " ",
        "--purpose",
        "p",
        "--owner-session",
        "s",
        "--owner-pid",
        "1",
    ]);
    let stderr = String::from_utf8_lossy(&empty.stderr).to_string();
    assert!(
        !empty.status.success() && stderr.contains("pass --creator, --purpose and --owner-session"),
        "an empty creator was registered: {stderr}"
    );
    let register = native.home.join(".stado/space/scratch.json");
    fs::create_dir_all(register.parent().unwrap()).unwrap();
    fs::write(&register, b"not a register").unwrap();
    let damaged = native.run(&["space", "scratch", "list"]);
    let stderr = String::from_utf8_lossy(&damaged.stderr).to_string();
    assert!(
        !damaged.status.success()
            && stderr.contains("is not a scratch register")
            && stderr.contains("move it aside")
            && stderr.contains("no registered tree is taken"),
        "a damaged register was read: {stderr}"
    );
}
