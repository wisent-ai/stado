//! The driver: one account per selected host, and the per-host outcome it
//! reports back.

use crate::deploy::{host_channel, DeployError, Runner};
use crate::targets::ComputeTarget;

use super::remote::{parse_status, remote_command};
use super::select::select_targets;
use super::validate::{validate_password, validate_shell, validate_text, validate_username};

/// Python `HostUserResult`: one host's outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostUserResult {
    pub target: String,
    pub ssh: String,
    pub status: String,
    pub os_name: String,
    pub detail: String,
}

impl HostUserResult {
    /// Python `ok` property: created / exists / planned all count.
    pub fn ok(&self) -> bool {
        matches!(self.status.as_str(), "created" | "exists" | "planned")
    }
}

/// Keyword options of Python `provision_users`.
#[derive(Debug, Default, Clone)]
pub struct ProvisionOptions<'a> {
    pub username: &'a str,
    pub password: Option<&'a str>,
    pub target_names: &'a [String],
    pub all_targets: bool,
    pub full_name: Option<&'a str>,
    pub shell: &'a str,
    pub admin: bool,
    pub require_password_change: bool,
    pub dry_run: bool,
}

/// Python `provision_users`: create one account on each selected registry
/// host and return every outcome.
pub async fn provision_users(
    options: &ProvisionOptions<'_>,
    targets: &[&ComputeTarget],
    runner: &Runner,
) -> Result<Vec<HostUserResult>, DeployError> {
    validate_username(options.username)?;
    let full_name = options.full_name.unwrap_or(options.username);
    validate_text(full_name, "full name", 255)?;
    validate_shell(options.shell)?;
    if !options.dry_run {
        let Some(password) = options.password else {
            return Err(DeployError("initial password is required".to_string()));
        };
        validate_password(password)?;
    }

    let selected = select_targets(targets, options.target_names, options.all_targets)?;

    let mut results: Vec<HostUserResult> = Vec::new();
    for target in selected {
        let name = target.name.clone();
        let mut ssh_target = target
            .ssh_connections()
            .next()
            .map_or_else(String::new, |(_, destination)| destination.to_string());
        if options.dry_run {
            results.push(HostUserResult {
                target: name,
                ssh: ssh_target,
                status: "planned".to_string(),
                os_name: String::new(),
                detail: String::new(),
            });
            continue;
        }

        let command = remote_command(
            options.username,
            full_name,
            options.shell,
            options.admin,
            options.require_password_change,
        );
        let password = format!("{}\n", options.password.unwrap_or(""));
        let completed = match host_channel::run_program_with_stdin_and_connection(
            target,
            &["/bin/sh", "-c", command.as_str()],
            &password,
            runner,
        )
        .await
        {
            Ok((completed, host_channel::UsedConnection::Ssh(connection))) => {
                ssh_target = connection.destination.to_string();
                completed
            }
            Ok((completed, host_channel::UsedConnection::Local)) => completed,
            Err(exc) => {
                results.push(HostUserResult {
                    target: name,
                    ssh: ssh_target,
                    status: "failed".to_string(),
                    os_name: String::new(),
                    detail: exc.0,
                });
                continue;
            }
        };

        if !completed.ok() {
            let detail_text = if !completed.stderr.is_empty() {
                completed.stderr.as_str()
            } else if !completed.stdout.is_empty() {
                completed.stdout.as_str()
            } else {
                ""
            };
            let exit_detail;
            let detail_text = if detail_text.is_empty() {
                exit_detail = format!("ssh exit {}", completed.code);
                exit_detail.as_str()
            } else {
                detail_text
            };
            let trimmed = detail_text.trim();
            let detail: String = trimmed
                .chars()
                .skip(trimmed.chars().count().saturating_sub(2000))
                .collect();
            results.push(HostUserResult {
                target: name,
                ssh: ssh_target,
                status: "failed".to_string(),
                os_name: String::new(),
                detail,
            });
            continue;
        }
        match parse_status(&completed.stdout, options.username) {
            Ok((status, os_name)) => results.push(HostUserResult {
                target: name,
                ssh: ssh_target,
                status,
                os_name,
                detail: String::new(),
            }),
            Err(exc) => results.push(HostUserResult {
                target: name,
                ssh: ssh_target,
                status: "failed".to_string(),
                os_name: String::new(),
                detail: exc.0,
            }),
        }
    }
    Ok(results)
}
