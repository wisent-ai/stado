//! Reading a host's Apple accounts, and the two parsers that answer for it.

use serde_json::Value;

use crate::deploy::Runner;
use crate::targets::ComputeTarget;

/// What a probe learned about a host's Apple accounts: the identifiers it
/// holds (an empty list is observed absence), or why the host could not be
/// asked, naming the step that did not answer. Unknown is never absent:
/// sending an operator to re-enroll a machine that is actually signed in is
/// the worse error.
pub(in crate::cli::identity) enum AccountObservation {
    Accounts(Vec<String>),
    Unknown(String),
}

/// Read a host's live Apple-account bindings through Stado's own approved channel.
///
/// Not `ssh`. A one-liner over ssh is the same action with the audit trail removed,
/// and this file previously did exactly that -- teaching the anti-pattern from inside
/// the tool meant to replace it. `host exec` runs one fixed, read-only, allowlisted
/// argv, so the probe cannot be pointed at a path and cannot grow into a shell.
///
/// The reading is the login user's own, because `defaults read` carries no path and
/// no sudo. A binding naming some other user on that machine is therefore reported
/// unknown rather than guessed at: an account signed into one login user says
/// nothing about whether another can display a prompt.
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
) -> AccountObservation {
    let runner = crate::deploy::production_runner();
    let words = vec![
        "defaults".to_string(),
        "read".to_string(),
        "MobileMeAccounts".to_string(),
    ];
    let report = match crate::deploy::host_exec::exec_host(target_name, &words, &runner).await {
        Ok(report) => report,
        Err(error) => {
            return AccountObservation::Unknown(format!(
                "{target_name} did not run `defaults read MobileMeAccounts`: {error}"
            ))
        }
    };
    if report.get("status").and_then(Value::as_str) != Some(crate::deploy::host_exec::OK_STATUS) {
        return AccountObservation::Unknown(format!(
            "{target_name} refused `defaults read MobileMeAccounts`: {report}"
        ));
    }
    let Some(stdout) = report.get("stdout").and_then(Value::as_str) else {
        return AccountObservation::Unknown(format!(
            "{target_name} ran `defaults read MobileMeAccounts` and its report carries no stdout"
        ));
    };
    let found = account_ids(stdout);
    if found.is_empty() {
        return AccountObservation::Unknown(format!(
            "{target_name} answered `defaults read MobileMeAccounts` without an AccountID; \
             the domain may be absent for the channel's user"
        ));
    }
    AccountObservation::Accounts(found)
}

/// Ask the host which of its users hold Apple accounts, read natively over the
/// host channel: one directory-service lookup, two file tests and one
/// `plutil -p`, with every branch taken here.
///
/// `defaults read` answers only for the user the channel logs in as, so a binding
/// naming anyone else could never be anything but `unknown`. That word covered two
/// opposite situations -- the user is not signed in, and this probe was not allowed
/// to look -- and an operator acts differently on each. The probe reports them
/// apart: an account list, `none`, or the step it could not take.
///
/// The probe rides Stado's own audited channel -- the one the retired helper
/// pair used to be the long way around -- so a host that cannot answer is one
/// the channel itself could not reach, and the answer says which call failed.
pub(in crate::cli::identity) async fn observe_user_apple_accounts(
    target_name: &str,
    user: &str,
) -> AccountObservation {
    let runner = crate::deploy::production_runner();
    let target = match crate::deploy::host_channel::canonical_target(target_name).await {
        Ok(target) => target,
        Err(error) => {
            return AccountObservation::Unknown(format!(
                "{target_name} is not a reachable registry target: {error}"
            ))
        }
    };

    // The host's directory service says where the user's home is; a name it
    // does not know, or knows without a home, is one the probe cannot look
    // at -- unknown, not absent. Names are not sorted by what they look
    // like: `dscl` decides.
    let home = match crate::deploy::host_channel::run_program(
        &target,
        &[
            "/usr/bin/dscl",
            ".",
            "-read",
            &format!("/Users/{user}"),
            "NFSHomeDirectory",
        ],
        &runner,
    )
    .await
    {
        Ok(home) => home,
        Err(error) => {
            return AccountObservation::Unknown(format!(
                "{target_name} did not look up {user:?} in its directory service: {error}"
            ))
        }
    };
    if !home.succeeded() {
        return AccountObservation::Unknown(format!(
            "{target_name}'s directory service does not know a user {user:?}: {}",
            home.stderr.trim()
        ));
    }
    let Some(home_directory) = home
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("NFSHomeDirectory:"))
        .map(str::trim)
        .filter(|path| !path.is_empty())
    else {
        return AccountObservation::Unknown(format!(
            "{target_name}'s directory service names no home directory for {user:?}"
        ));
    };

    // Read the preference file directly: the account identifiers it holds,
    // the step the channel may not take, or `none` when there is no such
    // file. Only the first and the last are observations; the middle one is
    // the probe admitting its limit, which is the distinction the whole thing
    // exists to make. Read-only throughout: a preference file is opened and
    // nothing is written anywhere.
    //
    // The order below is the correction. `test -f` inside another user's home
    // fails on macOS for lack of search permission -- homes are closed to
    // other users -- and this returned that as an empty list, which reads as
    // "that user is not signed in" about an account signed into that Mac for
    // weeks, so the Developer ID run refused with `no host holds
    // apple-account`. The `-r` test meant to catch it sat BEHIND the `-f`
    // test, so it could never fire. Absence is only claimed once the
    // directory has been shown to be searchable.
    let plist = format!("{home_directory}/Library/Preferences/MobileMeAccounts.plist");
    let quoted = crate::deploy::shlex_quote(&plist);
    let directory = crate::deploy::shlex_quote(&format!("{home_directory}/Library/Preferences"));
    let readable = match remote_test(&target, &format!("-r {quoted}"), &runner).await {
        Ok(readable) => readable,
        Err(step) => return step,
    };
    if readable {
        return match crate::deploy::host_channel::run_program(
            &target,
            &["/usr/bin/plutil", "-p", &plist],
            &runner,
        )
        .await
        {
            Ok(printed) => AccountObservation::Accounts(plutil_account_ids(&printed.stdout)),
            Err(error) => {
                AccountObservation::Unknown(format!("{target_name} did not print {plist}: {error}"))
            }
        };
    }
    // Not readable as the channel's user. The fleet already reaches root on
    // these hosts for `launchctl` and `install`, so the same grant answers
    // this question rather than leaving it to a guess; a host that does not
    // grant it says so.
    let privileged = match crate::deploy::host_channel::run_program(
        &target,
        &["/usr/bin/sudo", "-n", "/usr/bin/plutil", "-p", &plist],
        &runner,
    )
    .await
    {
        Ok(privileged) => privileged,
        Err(error) => {
            return AccountObservation::Unknown(format!(
                "{target_name} did not run `sudo -n plutil -p {plist}`: {error}"
            ))
        }
    };
    if privileged.succeeded() {
        return AccountObservation::Accounts(plutil_account_ids(&privileged.stdout));
    }
    // Neither read worked. Absence is a claim, and it is only made when the
    // channel could look at the directory and found no file there.
    let searchable = match remote_test(&target, &format!("-x {directory}"), &runner).await {
        Ok(searchable) => searchable,
        Err(step) => return step,
    };
    let present = match remote_test(&target, &format!("-f {quoted}"), &runner).await {
        Ok(present) => present,
        Err(step) => return step,
    };
    if searchable && !present {
        return AccountObservation::Accounts(Vec::new());
    }
    AccountObservation::Unknown(format!(
        "{target_name} holds {plist} but neither the channel's user nor `sudo -n` may read it: {}",
        privileged.stderr.trim()
    ))
}

/// One `test` on the host, or the observation that names the test the channel
/// could not run.
async fn remote_test(
    target: &ComputeTarget,
    expression: &str,
    runner: &Runner,
) -> Result<bool, AccountObservation> {
    crate::deploy::host_channel::remote_test(target, expression, runner)
        .await
        .map_err(|error| {
            AccountObservation::Unknown(format!(
                "{} did not answer `test {expression}`: {error}",
                target.name
            ))
        })
}

/// Every `AccountID` a `plutil -p` dump names, in the order printed.
///
/// A separate parser from [`account_ids`] on purpose: `defaults read` writes
/// `AccountID = "x";` and `plutil -p` writes `"AccountID" => "x"`, so the
/// value is the first quoted segment after the arrow.
fn plutil_account_ids(printed: &str) -> Vec<String> {
    printed
        .lines()
        .filter(|line| line.contains("AccountID"))
        .filter_map(|line| {
            let (_, value) = line.split_once("=>")?;
            let mut quoted = value.split('"');
            quoted.next();
            quoted.next().map(str::to_string)
        })
        .collect()
}
