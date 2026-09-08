//! Taking a lease: create the account, trust it, record it, enter it.

use crate::deploy::scratch::lease::ScratchLease;
use crate::deploy::scratch::remote;
use crate::deploy::{host_channel, host_users, DeployError, Runner};
use crate::targets::ComputeTarget;

/// The display name prefix a leased account carries, so `dscl` and `getent` say
/// what the account is for. The lease name is appended, and that is not
/// cosmetic: macOS refuses `sysadminctl -addUser` when another account already
/// carries the same full name, so one shared display name allowed exactly one
/// scratch account per mac. The refusal talks about a full name and never about
/// leases, so the limit was invisible until two runs wanted one host.
const FULL_NAME_PREFIX: &str = "Stado scratch";

/// Field index of the home directory in the `trusted` marker, whose fields are
/// the username and then the home the directory service reported.
const HOME_FIELD: u8 = 1;

/// Create the account through the channel account creation already uses.
pub async fn open_account(
    target: &ComputeTarget,
    name: &str,
    shell: &str,
    runner: &Runner,
) -> Result<(), DeployError> {
    let selection = vec![target.name.clone()];
    let password = one_time_password();
    let full_name = format!("{FULL_NAME_PREFIX} {name}");
    let options = host_users::ProvisionOptions {
        username: name,
        password: Some(&password),
        target_names: &selection,
        full_name: Some(&full_name),
        shell,
        ..host_users::ProvisionOptions::default()
    };
    let results = host_users::provision_users(&options, &[target], runner).await?;
    let Some(outcome) = results.first() else {
        return Err(DeployError(format!(
            "account creation reported nothing for '{name}'"
        )));
    };
    match outcome.status.as_str() {
        "created" => Ok(()),
        // An account with this name and no lease is a leak from an earlier run,
        // and adopting it silently would make this lease's history a guess.
        "exists" => Err(DeployError(format!(
            "account '{name}' already exists on '{}' outside any lease; \
             remove it with `stado host user delete {name} --target {}` before leasing this name",
            target.name, target.name
        ))),
        other => Err(DeployError(format!(
            "account '{name}' was not created on '{}': {}",
            target.name,
            if outcome.detail.is_empty() {
                other.to_string()
            } else {
                outcome.detail.clone()
            }
        ))),
    }
}

/// What entering a fresh lease proved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entered {
    pub login: String,
    pub home_path: String,
    pub exit_code: i32,
}

/// Trust the account with the keys that already reach the host, record the
/// lease, and enter it. The last step is the one that matters: a lease nobody
/// can log into is not a target, and every earlier step can succeed while that
/// is still true.
pub async fn settle(
    parent: &ComputeTarget,
    record: &ScratchLease,
    home: &str,
    leased: &ComputeTarget,
    runner: &Runner,
) -> Result<Entered, DeployError> {
    let source_keys = format!("{}/.ssh/authorized_keys", home.trim_end_matches('/'));
    let command = remote::trust_command(&record.username, &source_keys);
    let trusted =
        host_channel::run_program(parent, &["/bin/sh", "-c", command.as_str()], runner).await?;
    if !trusted.ok() {
        return Err(DeployError(format!(
            "'{}' could not be trusted with the keys that reach '{}': {}",
            record.username,
            parent.name,
            host_channel::last_error_line(&trusted, "the trust program printed nothing")
        )));
    }
    let marker = remote::parse_marker(&trusted.stdout, "trusted").unwrap_or_default();
    let account_home = marker
        .split('\t')
        .nth(HOME_FIELD.into())
        .unwrap_or_default()
        .to_string();

    let command = remote::record_command(record, home)?;
    let recorded =
        host_channel::run_program(parent, &["/bin/sh", "-c", command.as_str()], runner).await?;
    if !recorded.ok() {
        return Err(DeployError(format!(
            "the lease record for '{}' could not be written on '{}': {}",
            record.name,
            parent.name,
            host_channel::last_error_line(&recorded, "the record program printed nothing")
        )));
    }

    let entered = host_channel::run_program(leased, &["/usr/bin/id", "-un"], runner).await?;
    if !entered.ok() {
        return Err(DeployError(format!(
            "the leased target '{}' does not answer ssh: {}",
            leased.name,
            host_channel::last_error_line(&entered, "no output at all")
        )));
    }
    let login = entered.stdout.trim().to_string();
    if login != record.username {
        return Err(DeployError(format!(
            "ssh to the leased target answered as '{login}', not '{}'",
            record.username
        )));
    }
    Ok(Entered {
        login,
        home_path: account_home,
        exit_code: entered.code,
    })
}

/// A password used once, on ssh stdin, and never again: the account is entered
/// by key and destroyed with its lease. Never stored, never printed, never in
/// an argv.
fn one_time_password() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
