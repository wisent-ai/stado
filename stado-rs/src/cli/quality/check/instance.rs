//! The values this installation declares about itself, kept out of a commit.
//!
//! A host name, an account, a login or the path of an account's home written
//! into a product's repository tells every reader how one installation is
//! laid out, and code that branches on one decides the product's behaviour
//! by that installation. The values are read when the check runs, from the
//! registry this machine reads and from this account, and each checked
//! commit's added lines and message are refused when they carry one; none
//! of them is written here.

use std::path::Path;
use std::process::Command;

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

/// One value and what declares it.
struct Declared {
    value: String,
    by: String,
}

/// Every value this installation declares about itself: each registry
/// target's name, host names and ssh login (its host, and its account unless
/// this machine has an account of that name too, as every machine has
/// `root`: such an account names no installation), this account's home
/// directory and name.
///
/// The registry is the authority's when it answers. When it does not, the
/// last-known-good copy is read however old it is: the max-stale bound
/// guards decisions that act on where a host is now, and this is a list of
/// names to keep out of a commit, which an older copy still holds in full,
/// missing only a host added since. The copy's age is printed, so a check
/// read from it says so. No copy at all is the refusal.
async fn declared() -> Result<Vec<Declared>, String> {
    let registry = match crate::targets::fetch_registry_remote().await {
        Ok(registry) => registry,
        Err(authority) => {
            let (registry, notice) = crate::targets::registry_without_authority(
                "the quality check reads this installation's own values",
            )
            .await
            .map_err(|copy| {
                format!("the registry authority did not answer ({authority}), and {copy}")
            })?;
            if let Some(notice) = notice {
                eprintln!("{notice}, because the authority did not answer: {authority}");
            }
            registry
        }
    };
    let mut values = Vec::new();
    let mut add = |value: &str, by: String| {
        let value = value.trim();
        if !value.is_empty() {
            values.push(Declared {
                value: value.to_owned(),
                by,
            });
        }
    };
    for target in &registry.targets {
        add(&target.name, format!("registry target {}", target.name));
        for hostname in &target.hostnames {
            add(
                hostname,
                format!("registry target {}'s host name", target.name),
            );
        }
        if let Some(login) = &target.ssh {
            let (account, host) = login.split_once('@').unwrap_or(("", login.as_str()));
            if !account.is_empty() && !local_account(account) {
                add(
                    account,
                    format!("registry target {}'s ssh login", target.name),
                );
            }
            add(host, format!("registry target {}'s ssh login", target.name));
        }
    }
    let home = std::env::var("HOME").map_err(|error| format!("HOME cannot be read: {error}"))?;
    add(&home, "this account's home directory".to_owned());
    if let Some(name) = Path::new(&home).file_name().and_then(|name| name.to_str()) {
        add(name, "this account's name".to_owned());
    }
    Ok(values)
}

/// Whether this machine's account database knows `name`.
fn local_account(name: &str) -> bool {
    let Ok(name) = std::ffi::CString::new(name) else {
        return false;
    };
    // SAFETY: getpwnam reads the account database for a NUL-terminated name
    // and returns a pointer into static storage, which is only tested here.
    !unsafe { nix::libc::getpwnam(name.as_ptr()) }.is_null()
}

/// Whether `value` stands in `text` as a whole word: no letter, digit, `-`
/// or `_` touches it on either side, letter case aside.
fn stands_in(text: &str, value: &str) -> bool {
    let (text, value) = (text.to_lowercase(), value.to_lowercase());
    let word = |c: char| c.is_alphanumeric() || c == '-' || c == '_';
    text.match_indices(&value).any(|(at, found)| {
        let before = text[..at].chars().next_back();
        let after = text[at + found.len()..].chars().next();
        !before.is_some_and(word) && !after.is_some_and(word)
    })
}

fn git(checkout: &Path, args: &[&str]) -> Result<String, CmdError> {
    let output = crate::wait::output(Command::new("git").args(args).current_dir(checkout))
        .map_err(|error| CmdError::click(format!("cannot run git {}: {error}", args.join(" "))))?;
    if !output.status.success() {
        return Err(CmdError::click(format!(
            "git {} in {} failed: {}",
            args.join(" "),
            checkout.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Each added line of `revision` as `<file>:<line>` and its text.
fn added(checkout: &Path, revision: &str) -> Result<Vec<(String, String)>, CmdError> {
    let patch = git(
        checkout,
        &[
            "show",
            "--format=",
            "--unified=0",
            "--no-color",
            "--no-ext-diff",
            revision,
        ],
    )?;
    let (mut file, mut line, mut lines) = (String::new(), 0usize, Vec::new());
    for row in patch.lines() {
        if let Some(path) = row.strip_prefix("+++ ") {
            file = path.strip_prefix("b/").unwrap_or(path).to_owned();
        } else if let Some(hunk) = row.strip_prefix("@@ ") {
            // `@@ -a,b +c,d @@`: the new side starts at line c.
            line = hunk
                .split(' ')
                .find_map(|part| part.strip_prefix('+'))
                .and_then(|new| new.split(',').next())
                .and_then(|start| start.parse().ok())
                .ok_or_else(|| {
                    CmdError::click(format!(
                        "git show {revision} wrote an unreadable hunk header: {row}"
                    ))
                })?;
        } else if let Some(text) = row.strip_prefix('+') {
            lines.push((format!("{file}:{line}"), text.to_owned()));
            line += 1;
        }
    }
    Ok(lines)
}

/// Refuse `revision` of `checkout` when an added line or its message
/// carries a value this installation declares about itself, naming each.
pub(super) async fn refuse_declared(checkout: &Path, revision: &str) -> Result<(), CmdError> {
    let values = declared().await.map_err(|cause| {
        CmdError::click(format!(
            "stado quality check: this installation's own values cannot be read, so {revision} is not \
             known to be free of them: {cause}"
        ))
        .stating(FailureCode::InfraDown)
    })?;
    let message = git(checkout, &["log", "-1", "--format=%B", revision])?;
    let mut places = vec![("commit message".to_owned(), message)];
    places.extend(added(checkout, revision)?);
    let found: Vec<String> = places
        .iter()
        .flat_map(|(place, text)| {
            values
                .iter()
                .filter(|declared| stands_in(text, &declared.value))
                .map(move |declared| format!("  {place}: `{}` ({})", declared.value, declared.by))
        })
        .collect();
    if found.is_empty() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "stado quality check: {revision} carries this installation's own values, which a commit must not hold:\n{}\n\
         Write the general rule the code needs, or read the value from where this installation declares it, \
         then commit again.",
        found.join("\n")
    ))
    .stating(FailureCode::Refused))
}
