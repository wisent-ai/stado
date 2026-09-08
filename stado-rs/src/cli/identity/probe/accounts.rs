//! Reading a host's Apple accounts, and the two parsers that answer for it.

use serde_json::Value;

/// Read a host's live Apple-account bindings through Stado's own approved channel.
///
/// Not `ssh`. A one-liner over ssh is the same action with the audit trail removed,
/// and this file previously did exactly that -- teaching the anti-pattern from inside
/// the tool meant to replace it. `host exec` runs one fixed, read-only, allowlisted
/// argv, so the probe cannot be pointed at a path and cannot grow into a shell.
///
/// The reading is the login user's own, because `defaults read` carries no path and
/// no sudo. A binding naming some other user on that machine is therefore reported
/// unknown rather than guessed at: an account signed into `charles` says nothing
/// about whether `weles-apple` can display a prompt.
///
/// `None` means the probe could not run -- unreachable host, refused channel, no such
/// domain. That is unknown, never absent, because sending an operator to re-enroll a
/// machine that is actually signed in is the worse error.
pub(super) fn account_ids(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| line.contains("AccountID"))
        .filter_map(|line| {
            let mut quoted = line.split('"');
            quoted.next();
            quoted.next().map(str::to_string)
        })
        .collect()
}

pub(in crate::cli::identity) async fn observe_apple_accounts(
    target_name: &str,
) -> Option<Vec<String>> {
    let runner = crate::deploy::production_runner();
    let words = vec![
        "defaults".to_string(),
        "read".to_string(),
        "MobileMeAccounts".to_string(),
    ];
    let report = crate::deploy::host_exec::exec_host(target_name, &words, &runner)
        .await
        .ok()?;
    if report.get("status").and_then(Value::as_str) != Some(crate::deploy::host_exec::OK_STATUS) {
        return None;
    }
    let found = account_ids(report.get("stdout").and_then(Value::as_str)?);
    if found.is_empty() {
        None
    } else {
        Some(found)
    }
}

/// Ask the host which of its users hold Apple accounts, read natively over the
/// host channel: one directory listing, two file tests and one `plutil -p`,
/// with every branch taken here.
///
/// `defaults read` answers only for the user the channel logs in as, so a binding
/// naming anyone else could never be anything but `unknown`. That word covered two
/// opposite situations -- the user is not signed in, and this probe was not allowed
/// to look -- and an operator acts differently on each. The probe reports them
/// apart: an account list, `none`, or `unreadable`.
///
/// The probe rides Stado's own audited channel -- the one the retired helper
/// pair used to be the long way around -- so a host that cannot answer is one
/// the channel itself could not reach, and the answer stays `unknown`, which
/// is the honest answer when nothing on that host can produce a better one.
pub(in crate::cli::identity) async fn observe_user_apple_accounts(
    target_name: &str,
    user: &str,
) -> Option<Vec<String>> {
    let runner = crate::deploy::production_runner();
    let target = crate::deploy::host_channel::canonical_target(target_name)
        .await
        .ok()?;

    // The probe's per-user walk, narrowed to the one user the binding names:
    // `ls /Users` decides which homes the host has, and `Shared`, `Guest` and
    // dot-directories are not users. A user with no home directory on that
    // host is a user the probe could not look at -- unknown, not absent.
    let homes = crate::deploy::host_channel::run_program(&target, &["/bin/ls", "/Users"], &runner)
        .await
        .ok()?;
    if !homes.ok()
        || user == "Shared"
        || user == "Guest"
        || user.starts_with('.')
        || !homes.stdout.lines().any(|name| name == user)
    {
        return None;
    }

    // Read the preference file directly: the account identifiers it holds,
    // `unreadable` when the channel may not open it, or `none` when there is
    // no such file. Only the first and the last are observations; the middle
    // one is the probe admitting its limit, which is the distinction the whole
    // thing exists to make. Read-only throughout: a preference file is opened
    // and nothing is written anywhere.
    //
    // The order below is the correction. `test -f` inside another user's home
    // fails on macOS for lack of search permission — homes are 700 — and this
    // returned that as `Some(vec![])`, which reads as "that user is not signed
    // in". On 2026-09-04 it said exactly that about an account the operator
    // had been signed into on that Mac for weeks, and the Developer ID run
    // refused with `no host holds apple-account`. The `-r` test meant to catch
    // it sat BEHIND the `-f` test, so it could never fire. Absence is only
    // claimed once the directory has been shown to be searchable.
    let plist = format!("/Users/{user}/Library/Preferences/MobileMeAccounts.plist");
    let quoted = crate::deploy::shlex_quote(&plist);
    let directory = crate::deploy::shlex_quote(&format!("/Users/{user}/Library/Preferences"));
    let readable =
        crate::deploy::host_channel::remote_test(&target, &format!("-r {quoted}"), &runner)
            .await
            .ok()?;
    if readable {
        let printed = crate::deploy::host_channel::run_program(
            &target,
            &["/usr/bin/plutil", "-p", &plist],
            &runner,
        )
        .await
        .ok()?;
        return Some(plutil_account_ids(&printed.stdout));
    }
    // Not readable as the channel's user. The fleet already reaches root on
    // these hosts for `launchctl` and `install`, so the same grant answers
    // this question rather than leaving it to a guess; a host that does not
    // grant it stays unknown.
    let privileged = crate::deploy::host_channel::run_program(
        &target,
        &["/usr/bin/sudo", "-n", "/usr/bin/plutil", "-p", &plist],
        &runner,
    )
    .await
    .ok()?;
    if privileged.ok() {
        return Some(plutil_account_ids(&privileged.stdout));
    }
    // Neither read worked. Absence is a claim, and it is only made when the
    // channel could look at the directory and found no file there.
    let searchable =
        crate::deploy::host_channel::remote_test(&target, &format!("-x {directory}"), &runner)
            .await
            .ok()?;
    let present =
        crate::deploy::host_channel::remote_test(&target, &format!("-f {quoted}"), &runner)
            .await
            .ok()?;
    if searchable && !present {
        return Some(Vec::new());
    }
    None
}

/// Every `AccountID` a `plutil -p` dump names, in the order printed.
///
/// A separate parser from [`account_ids`] on purpose: `defaults read` writes
/// `AccountID = "x";` and `plutil -p` writes `"AccountID" => "x"`, so one
/// quoted-segment index cannot read both.
fn plutil_account_ids(printed: &str) -> Vec<String> {
    printed
        .lines()
        .filter(|line| line.contains("AccountID"))
        .filter_map(|line| line.split('"').nth(3).map(str::to_string))
        .collect()
}
