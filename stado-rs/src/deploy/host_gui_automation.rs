//! Managed CuaDriver lifecycle for a registry-owned macOS host.
//!
//! The host command owns the complete reusable path: install one pinned,
//! checksummed and signed CuaDriver release, register its app bundle, grant the
//! login user's Accessibility row, report the resulting state, and remove it.
//! Every remote action is one fixed program invocation through `host_channel`;
//! no helper or shell script is installed or carried in this binary.

use crate::deploy::{host_channel, CommandOutput, DeployError, Runner};
use crate::targets::ComputeTarget;

pub const CUA_DRIVER_VERSION: &str = "0.22.0";
pub const CUA_DRIVER_BUNDLE_ID: &str = "com.trycua.driver";
pub const CUA_DRIVER_APP: &str = "/Applications/CuaDriver.app";
pub const CUA_DRIVER_ARCHIVE_SHA256: &str =
    "59603bc7e5f8d9d70f165d87158e577f99227ffcbb91d5fd9f9c688f4beb3727";
pub const CUA_DRIVER_ARCHIVE_URL: &str = "https://github.com/trycua/cua/releases/download/\
    cua-driver-rs-v0.22.0/cua-driver-rs-0.22.0-darwin-universal.tar.gz";

const PLIST_BUDDY: &str = "/usr/libexec/PlistBuddy";
const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/\
    LaunchServices.framework/Versions/A/Support/lsregister";
const KICKSTART: &str = "/System/Library/CoreServices/RemoteManagement/ARDAgent.app/Contents/\
    Resources/kickstart";
const REMOTE_MANAGEMENT_PREFS: &str = "/Library/Preferences/com.apple.RemoteManagement";
const ACCESSIBILITY_SERVICE: &str = "kTCCServiceAccessibility";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuiAutomationReport {
    pub target: String,
    pub ssh_target: String,
    pub items: Vec<(String, String)>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AppIdentity {
    bundle: String,
    version: String,
    requirement: String,
}

fn report(
    target: &ComputeTarget,
    items: Vec<(String, String)>,
    result: Result<(), DeployError>,
) -> GuiAutomationReport {
    GuiAutomationReport {
        target: target.name.clone(),
        ssh_target: target.ssh.clone().unwrap_or_default(),
        items,
        error: result.err().map(|error| error.0),
    }
}

fn require_target(target: &ComputeTarget) -> Result<(), DeployError> {
    if target.ssh.as_deref().unwrap_or_default().is_empty() {
        return Err(DeployError(format!(
            "target {} has no ssh destination in the registry",
            target.name
        )));
    }
    if target.release_platform != "darwin-arm64" {
        return Err(DeployError(format!(
            "target {} is {:?}; GUI automation requires darwin-arm64",
            target.name, target.release_platform
        )));
    }
    Ok(())
}

fn safe_identity(value: &str, kind: &str) -> Result<(), DeployError> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(DeployError(format!("invalid {kind} {value:?}")));
    }
    Ok(())
}

async fn run(
    target: &ComputeTarget,
    words: &[&str],
    what: &str,
    runner: &Runner,
) -> Result<CommandOutput, DeployError> {
    let output = host_channel::run_program(target, words, runner).await?;
    if output.ok() {
        Ok(output)
    } else {
        Err(DeployError(format!(
            "{}: {what} failed: {}",
            target.name,
            output.detail().trim()
        )))
    }
}

async fn run_sudo(
    target: &ComputeTarget,
    words: &[&str],
    what: &str,
    runner: &Runner,
) -> Result<CommandOutput, DeployError> {
    let mut command = Vec::with_capacity(words.len() + 2);
    command.extend(["/usr/bin/sudo", "-n"]);
    command.extend(words.iter().copied());
    run(target, &command, what, runner).await
}

async fn optional(
    target: &ComputeTarget,
    words: &[&str],
    runner: &Runner,
) -> Result<Option<String>, DeployError> {
    let output = host_channel::run_program(target, words, runner).await?;
    Ok(output.ok().then(|| output.stdout.trim().to_string()))
}

async fn optional_sudo(
    target: &ComputeTarget,
    words: &[&str],
    runner: &Runner,
) -> Result<Option<String>, DeployError> {
    let mut command = Vec::with_capacity(words.len() + 2);
    command.extend(["/usr/bin/sudo", "-n"]);
    command.extend(words.iter().copied());
    optional(target, &command, runner).await
}

fn designated_requirement(output: &CommandOutput) -> Result<String, DeployError> {
    output
        .stderr
        .lines()
        .chain(output.stdout.lines())
        .find_map(|line| {
            line.split_once("designated => ")
                .map(|(_, value)| value.trim())
        })
        .map(str::to_string)
        .filter(|requirement| !requirement.is_empty())
        .ok_or_else(|| {
            DeployError(format!(
                "CuaDriver has no designated code requirement: {}",
                output.detail().trim()
            ))
        })
}

async fn app_identity(
    target: &ComputeTarget,
    app: &str,
    runner: &Runner,
) -> Result<Option<AppIdentity>, DeployError> {
    if !host_channel::remote_test(target, &format!("-d {}", super::shlex_quote(app)), runner)
        .await?
    {
        return Ok(None);
    }
    run(
        target,
        &["/usr/bin/codesign", "--verify", "--deep", "--strict", app],
        "CuaDriver signature verification",
        runner,
    )
    .await?;
    let plist = format!("{app}/Contents/Info.plist");
    let bundle = run(
        target,
        &[PLIST_BUDDY, "-c", "Print :CFBundleIdentifier", &plist],
        "CuaDriver bundle identity read",
        runner,
    )
    .await?
    .stdout
    .trim()
    .to_string();
    safe_identity(&bundle, "bundle identifier")?;
    let version = run(
        target,
        &[
            PLIST_BUDDY,
            "-c",
            "Print :CFBundleShortVersionString",
            &plist,
        ],
        "CuaDriver version read",
        runner,
    )
    .await?
    .stdout
    .trim()
    .to_string();
    safe_identity(&version, "CuaDriver version")?;
    let requirement = designated_requirement(
        &run(
            target,
            &["/usr/bin/codesign", "-d", "-r-", app],
            "CuaDriver code requirement read",
            runner,
        )
        .await?,
    )?;
    Ok(Some(AppIdentity {
        bundle,
        version,
        requirement,
    }))
}

async fn login_user(target: &ComputeTarget, runner: &Runner) -> Result<String, DeployError> {
    let login = run(
        target,
        &["/usr/bin/id", "-un"],
        "remote login user read",
        runner,
    )
    .await?
    .stdout
    .trim()
    .to_string();
    let console = optional(
        target,
        &["/usr/bin/stat", "-f", "%Su", "/dev/console"],
        runner,
    )
    .await?
    .unwrap_or_default();
    let user = match console.as_str() {
        "" | "root" | "loginwindow" | "_mbsetupuser" => login,
        _ => console,
    };
    safe_identity(&user, "GUI user")?;
    if matches!(user.as_str(), "root" | "loginwindow" | "_mbsetupuser") {
        return Err(DeployError("the host has no non-root GUI user".to_string()));
    }
    Ok(user)
}

async fn remove_if_present(
    target: &ComputeTarget,
    path: &str,
    privileged: bool,
    runner: &Runner,
) -> Result<(), DeployError> {
    if !host_channel::remote_test(target, &format!("-e {}", super::shlex_quote(path)), runner)
        .await?
    {
        return Ok(());
    }
    if privileged {
        run_sudo(
            target,
            &["/bin/rm", "-rf", path],
            "remove stale CuaDriver path",
            runner,
        )
        .await?;
    } else {
        run(
            target,
            &["/bin/rm", "-rf", path],
            "remove stale CuaDriver path",
            runner,
        )
        .await?;
    }
    Ok(())
}

async fn rollback_app(
    target: &ComputeTarget,
    backup: &str,
    runner: &Runner,
) -> Result<(), DeployError> {
    let _ = run_sudo(
        target,
        &["/bin/rm", "-rf", CUA_DRIVER_APP],
        "remove failed CuaDriver install",
        runner,
    )
    .await;
    if host_channel::remote_test(
        target,
        &format!("-e {}", super::shlex_quote(backup)),
        runner,
    )
    .await?
    {
        run_sudo(
            target,
            &["/bin/mv", backup, CUA_DRIVER_APP],
            "restore prior CuaDriver app",
            runner,
        )
        .await?;
    }
    Ok(())
}

async fn reconcile_app(
    target: &ComputeTarget,
    items: &mut Vec<(String, String)>,
    runner: &Runner,
) -> Result<(), DeployError> {
    require_target(target)?;
    if let Some(identity) = app_identity(target, CUA_DRIVER_APP, runner).await? {
        if identity.bundle == CUA_DRIVER_BUNDLE_ID && identity.version == CUA_DRIVER_VERSION {
            items.push(("cua-driver-app".to_string(), "reused".to_string()));
            items.push(("cua-driver-version".to_string(), identity.version));
            return Ok(());
        }
    }

    let home = host_channel::remote_home(target, runner).await?;
    let cache = format!("{home}/.stado/cache/cua-driver/{CUA_DRIVER_VERSION}");
    let archive = format!("{cache}/cua-driver.tar.gz");
    let partial = format!("{archive}.partial");
    let stage = format!("{cache}/stage");
    let stage_app =
        format!("{stage}/cua-driver-rs-{CUA_DRIVER_VERSION}-darwin-universal/CuaDriver.app");
    let backup = format!("{CUA_DRIVER_APP}.stado-backup");

    run(
        target,
        &["/bin/mkdir", "-p", &cache],
        "create CuaDriver cache",
        runner,
    )
    .await?;
    let archive_valid = if host_channel::remote_test(
        target,
        &format!("-f {}", super::shlex_quote(&archive)),
        runner,
    )
    .await?
    {
        let digest = run(
            target,
            &["/usr/bin/openssl", "dgst", "-sha256", "-r", &archive],
            "cached CuaDriver digest",
            runner,
        )
        .await?
        .stdout;
        digest.split_whitespace().next() == Some(CUA_DRIVER_ARCHIVE_SHA256)
    } else {
        false
    };
    if !archive_valid {
        remove_if_present(target, &partial, false, runner).await?;
        run(
            target,
            &[
                "/usr/bin/curl",
                "-fL",
                "--retry",
                "3",
                "--output",
                &partial,
                CUA_DRIVER_ARCHIVE_URL,
            ],
            "download pinned CuaDriver release",
            runner,
        )
        .await?;
        let digest = run(
            target,
            &["/usr/bin/openssl", "dgst", "-sha256", "-r", &partial],
            "downloaded CuaDriver digest",
            runner,
        )
        .await?
        .stdout;
        if digest.split_whitespace().next() != Some(CUA_DRIVER_ARCHIVE_SHA256) {
            remove_if_present(target, &partial, false, runner).await?;
            return Err(DeployError(
                "downloaded CuaDriver archive digest does not match the pinned release".to_string(),
            ));
        }
        run(
            target,
            &["/bin/mv", &partial, &archive],
            "commit CuaDriver archive",
            runner,
        )
        .await?;
    }

    remove_if_present(target, &stage, false, runner).await?;
    run(
        target,
        &["/bin/mkdir", "-p", &stage],
        "create CuaDriver staging directory",
        runner,
    )
    .await?;
    run(
        target,
        &["/usr/bin/tar", "-xzf", &archive, "-C", &stage],
        "extract CuaDriver release",
        runner,
    )
    .await?;
    let staged = app_identity(target, &stage_app, runner)
        .await?
        .ok_or_else(|| DeployError("CuaDriver release contains no app bundle".to_string()))?;
    if staged.bundle != CUA_DRIVER_BUNDLE_ID || staged.version != CUA_DRIVER_VERSION {
        return Err(DeployError(format!(
            "CuaDriver release identity is {} {}, expected {} {}",
            staged.bundle, staged.version, CUA_DRIVER_BUNDLE_ID, CUA_DRIVER_VERSION
        )));
    }

    if host_channel::remote_test(
        target,
        &format!("-e {}", super::shlex_quote(&backup)),
        runner,
    )
    .await?
    {
        if let Some(installed) = app_identity(target, CUA_DRIVER_APP, runner).await? {
            if installed.bundle == CUA_DRIVER_BUNDLE_ID && installed.version == CUA_DRIVER_VERSION {
                remove_if_present(target, &backup, true, runner).await?;
            } else {
                rollback_app(target, &backup, runner).await?;
            }
        } else {
            rollback_app(target, &backup, runner).await?;
        }
    }
    if host_channel::remote_test(
        target,
        &format!("-e {}", super::shlex_quote(CUA_DRIVER_APP)),
        runner,
    )
    .await?
    {
        run_sudo(
            target,
            &["/bin/mv", CUA_DRIVER_APP, &backup],
            "back up prior CuaDriver app",
            runner,
        )
        .await?;
    }
    if let Err(error) = run_sudo(
        target,
        &["/usr/bin/ditto", &stage_app, CUA_DRIVER_APP],
        "install CuaDriver app",
        runner,
    )
    .await
    {
        rollback_app(target, &backup, runner).await?;
        return Err(error);
    }
    let installed = match app_identity(target, CUA_DRIVER_APP, runner).await {
        Ok(Some(identity)) => identity,
        Ok(None) => {
            rollback_app(target, &backup, runner).await?;
            return Err(DeployError(
                "installed CuaDriver app is missing".to_string(),
            ));
        }
        Err(error) => {
            rollback_app(target, &backup, runner).await?;
            return Err(error);
        }
    };
    if installed != staged {
        rollback_app(target, &backup, runner).await?;
        return Err(DeployError(
            "installed CuaDriver app did not preserve its signed identity".to_string(),
        ));
    }
    if let Err(error) = run(
        target,
        &[LSREGISTER, "-f", CUA_DRIVER_APP],
        "register CuaDriver with LaunchServices",
        runner,
    )
    .await
    {
        rollback_app(target, &backup, runner).await?;
        return Err(error);
    }
    remove_if_present(target, &backup, true, runner).await?;
    let bin_dir = format!("{home}/.local/bin");
    let bin_link = format!("{bin_dir}/cua-driver");
    let app_binary = format!("{CUA_DRIVER_APP}/Contents/MacOS/cua-driver");
    run(
        target,
        &["/bin/mkdir", "-p", &bin_dir],
        "create CuaDriver binary directory",
        runner,
    )
    .await?;
    run(
        target,
        &["/bin/ln", "-sfn", &app_binary, &bin_link],
        "publish CuaDriver binary link",
        runner,
    )
    .await?;
    remove_if_present(target, &stage, false, runner).await?;
    items.push(("cua-driver-app".to_string(), "installed".to_string()));
    items.push((
        "cua-driver-version".to_string(),
        CUA_DRIVER_VERSION.to_string(),
    ));
    items.push((
        "cua-driver-sha256".to_string(),
        CUA_DRIVER_ARCHIVE_SHA256.to_string(),
    ));
    Ok(())
}

async fn grant_accessibility_inner(
    target: &ComputeTarget,
    items: &mut Vec<(String, String)>,
    runner: &Runner,
) -> Result<(), DeployError> {
    require_target(target)?;
    let identity = app_identity(target, CUA_DRIVER_APP, runner)
        .await?
        .ok_or_else(|| DeployError("CuaDriver.app is not installed".to_string()))?;
    if identity.bundle != CUA_DRIVER_BUNDLE_ID {
        return Err(DeployError(format!(
            "CuaDriver.app has bundle id {}, expected {}",
            identity.bundle, CUA_DRIVER_BUNDLE_ID
        )));
    }
    let user = login_user(target, runner).await?;
    let home = format!("/Users/{user}");
    let database = format!("{home}/Library/Application Support/com.apple.TCC/TCC.db");
    run_sudo(
        target,
        &["/bin/test", "-f", &database],
        "locate the GUI user's TCC database",
        runner,
    )
    .await?;
    let columns = run_sudo(
        target,
        &[
            "/usr/bin/sqlite3",
            &database,
            "SELECT group_concat(name, ',') FROM pragma_table_info('access');",
        ],
        "read TCC schema",
        runner,
    )
    .await?
    .stdout;
    for required in [
        "service",
        "client",
        "client_type",
        "auth_value",
        "auth_reason",
        "auth_version",
        "csreq",
        "indirect_object_identifier_type",
        "indirect_object_identifier",
        "flags",
        "last_modified",
    ] {
        if !columns.split(',').any(|column| column.trim() == required) {
            return Err(DeployError(format!(
                "the host's TCC schema has no {required} column"
            )));
        }
    }

    let cache = format!("{home}/.stado/cache/cua-driver");
    let requirement_file = format!("{cache}/accessibility.csreq");
    run(
        target,
        &["/bin/mkdir", "-p", &cache],
        "create CuaDriver cache",
        runner,
    )
    .await?;
    remove_if_present(target, &requirement_file, false, runner).await?;
    let requirement_argument = format!("={}", identity.requirement);
    run(
        target,
        &[
            "/usr/bin/csreq",
            "-r",
            &requirement_argument,
            "-b",
            &requirement_file,
        ],
        "compile CuaDriver code requirement",
        runner,
    )
    .await?;
    let csreq = run(
        target,
        &["/usr/bin/xxd", "-p", &requirement_file],
        "encode CuaDriver code requirement",
        runner,
    )
    .await?
    .stdout
    .split_whitespace()
    .collect::<String>();
    remove_if_present(target, &requirement_file, false, runner).await?;
    if csreq.is_empty() || !csreq.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(DeployError(
            "compiled CuaDriver code requirement is invalid".to_string(),
        ));
    }

    let backup_dir = format!("{home}/.stado/backups");
    let backup = format!("{backup_dir}/TCC.db.before-stado-accessibility");
    run(
        target,
        &["/bin/mkdir", "-p", &backup_dir],
        "create TCC backup directory",
        runner,
    )
    .await?;
    if !host_channel::remote_test(
        target,
        &format!("-f {}", super::shlex_quote(&backup)),
        runner,
    )
    .await?
    {
        let backup_command = format!(".backup '{}'", backup.replace('\'', "''"));
        run_sudo(
            target,
            &["/usr/bin/sqlite3", &database, &backup_command],
            "back up the TCC database",
            runner,
        )
        .await?;
        run_sudo(
            target,
            &["/usr/sbin/chown", &format!("{user}:staff"), &backup],
            "set TCC backup owner",
            runner,
        )
        .await?;
        run_sudo(
            target,
            &["/bin/chmod", "600", &backup],
            "set TCC backup mode",
            runner,
        )
        .await?;
    }
    let sql = format!(
        "BEGIN IMMEDIATE; DELETE FROM access WHERE service = '{ACCESSIBILITY_SERVICE}' AND client = '{}' AND client_type = 0; INSERT INTO access (service, client, client_type, auth_value, auth_reason, auth_version, csreq, policy_id, indirect_object_identifier_type, indirect_object_identifier, indirect_object_code_identity, flags, last_modified) VALUES ('{ACCESSIBILITY_SERVICE}', '{}', 0, 2, 3, 1, X'{}', NULL, 0, 'UNUSED', NULL, 0, strftime('%s','now')); COMMIT;",
        identity.bundle, identity.bundle, csreq
    );
    run_sudo(
        target,
        &["/usr/bin/sqlite3", &database, &sql],
        "grant CuaDriver Accessibility",
        runner,
    )
    .await?;
    let verify_sql = format!(
        "SELECT auth_value FROM access WHERE service = '{ACCESSIBILITY_SERVICE}' AND client = '{}' AND client_type = 0 ORDER BY last_modified DESC LIMIT 1;",
        identity.bundle
    );
    let granted = run_sudo(
        target,
        &["/usr/bin/sqlite3", &database, &verify_sql],
        "verify CuaDriver Accessibility",
        runner,
    )
    .await?
    .stdout;
    if granted.trim() != "2" {
        return Err(DeployError(
            "the CuaDriver Accessibility grant was not read back".to_string(),
        ));
    }
    items.push(("accessibility".to_string(), "granted".to_string()));
    items.push(("accessibility-user".to_string(), user));
    items.push(("accessibility-client".to_string(), identity.bundle));
    items.push(("accessibility-backup".to_string(), backup));
    Ok(())
}

async fn status_inner(
    target: &ComputeTarget,
    items: &mut Vec<(String, String)>,
    runner: &Runner,
) -> Result<(), DeployError> {
    require_target(target)?;
    let autologin = optional_sudo(
        target,
        &[
            "/usr/bin/defaults",
            "read",
            "/Library/Preferences/com.apple.loginwindow",
            "autoLoginUser",
        ],
        runner,
    )
    .await?
    .filter(|value| !value.is_empty())
    .unwrap_or_else(|| "none".to_string());
    items.push(("autologin".to_string(), autologin));
    let kcpassword = optional_sudo(target, &["/bin/test", "-f", "/etc/kcpassword"], runner)
        .await?
        .is_some();
    items.push((
        "kcpassword".to_string(),
        if kcpassword { "present" } else { "absent" }.to_string(),
    ));
    let ard = optional_sudo(
        target,
        &[
            "/usr/bin/defaults",
            "read",
            REMOTE_MANAGEMENT_PREFS,
            "ARD_AllLocalUsers",
        ],
        runner,
    )
    .await?
    .filter(|value| !value.is_empty())
    .unwrap_or_else(|| "unset".to_string());
    items.push(("remote-management-all-users".to_string(), ard));
    let vnc = optional_sudo(
        target,
        &[
            "/usr/bin/defaults",
            "read",
            REMOTE_MANAGEMENT_PREFS,
            "VNCLegacyConnectionsEnabled",
        ],
        runner,
    )
    .await?
    .filter(|value| !value.is_empty())
    .unwrap_or_else(|| "unset".to_string());
    items.push(("vnc-legacy".to_string(), vnc));
    let console = optional(
        target,
        &["/usr/bin/stat", "-f", "%Su", "/dev/console"],
        runner,
    )
    .await?
    .unwrap_or_else(|| "unknown".to_string());
    items.push(("console".to_string(), console));

    let Some(identity) = app_identity(target, CUA_DRIVER_APP, runner).await? else {
        items.push(("cua-driver-app".to_string(), "absent".to_string()));
        items.push(("accessibility".to_string(), "app-missing".to_string()));
        return Ok(());
    };
    items.push(("cua-driver-app".to_string(), "present".to_string()));
    items.push(("cua-driver-version".to_string(), identity.version));
    items.push(("cua-driver-client".to_string(), identity.bundle.clone()));
    let user = login_user(target, runner).await?;
    let database = format!("/Users/{user}/Library/Application Support/com.apple.TCC/TCC.db");
    let query = format!(
        "SELECT auth_value FROM access WHERE service = '{ACCESSIBILITY_SERVICE}' AND client = '{}' AND client_type = 0 ORDER BY last_modified DESC LIMIT 1;",
        identity.bundle
    );
    let value = optional_sudo(target, &["/usr/bin/sqlite3", &database, &query], runner)
        .await?
        .unwrap_or_default();
    let state = match value.trim() {
        "2" => "granted".to_string(),
        "" => "not-set".to_string(),
        other => format!("refused:{other}"),
    };
    items.push(("accessibility".to_string(), state));
    items.push(("accessibility-user".to_string(), user));
    Ok(())
}

async fn disable_inner(
    target: &ComputeTarget,
    bundle: &str,
    items: &mut Vec<(String, String)>,
    runner: &Runner,
) -> Result<(), DeployError> {
    require_target(target)?;
    if optional_sudo(
        target,
        &[
            "/usr/bin/defaults",
            "read",
            "/Library/Preferences/com.apple.loginwindow",
            "autoLoginUser",
        ],
        runner,
    )
    .await?
    .is_some()
    {
        let _ = run_sudo(
            target,
            &[
                "/usr/bin/defaults",
                "delete",
                "/Library/Preferences/com.apple.loginwindow",
                "autoLoginUser",
            ],
            "clear autologin",
            runner,
        )
        .await;
        items.push(("autologin".to_string(), "removed".to_string()));
    } else {
        items.push(("autologin".to_string(), "absent".to_string()));
    }
    if run_sudo(
        target,
        &["/bin/test", "-f", "/etc/kcpassword"],
        "read kcpassword state",
        runner,
    )
    .await
    .is_ok()
    {
        run_sudo(
            target,
            &["/bin/rm", "-f", "/etc/kcpassword"],
            "remove kcpassword",
            runner,
        )
        .await?;
        items.push(("kcpassword".to_string(), "removed".to_string()));
    } else {
        items.push(("kcpassword".to_string(), "absent".to_string()));
    }
    let _ = run_sudo(
        target,
        &[KICKSTART, "-deactivate", "-configure", "-access", "-off"],
        "deactivate Remote Management",
        runner,
    )
    .await;
    let _ = run_sudo(
        target,
        &[
            KICKSTART,
            "-configure",
            "-clientopts",
            "-setvnclegacy",
            "-vnclegacy",
            "no",
        ],
        "disable legacy VNC",
        runner,
    )
    .await;
    for key in [
        "ARD_AllLocalUsers",
        "ARD_AllLocalUsersPrivs",
        "VNCLegacyConnectionsEnabled",
    ] {
        let _ = run_sudo(
            target,
            &["/usr/bin/defaults", "delete", REMOTE_MANAGEMENT_PREFS, key],
            "clear Remote Management preference",
            runner,
        )
        .await;
    }
    items.push(("remote-management".to_string(), "deactivated".to_string()));
    if !bundle.is_empty() {
        safe_identity(bundle, "bundle identifier")?;
        let user = login_user(target, runner).await?;
        let database = format!("/Users/{user}/Library/Application Support/com.apple.TCC/TCC.db");
        let sql = format!("DELETE FROM access WHERE client = '{bundle}';");
        run_sudo(
            target,
            &["/usr/bin/sqlite3", &database, &sql],
            "revoke Accessibility",
            runner,
        )
        .await?;
        items.push(("tcc-revoked".to_string(), bundle.to_string()));
    }
    let home = host_channel::remote_home(target, runner).await?;
    remove_if_present(target, CUA_DRIVER_APP, true, runner).await?;
    remove_if_present(
        target,
        &format!("{home}/.local/bin/cua-driver"),
        false,
        runner,
    )
    .await?;
    remove_if_present(
        target,
        &format!("{home}/.stado/cache/cua-driver"),
        false,
        runner,
    )
    .await?;
    items.push(("cua-driver-app".to_string(), "removed".to_string()));
    Ok(())
}

pub async fn status(target: &ComputeTarget, runner: &Runner) -> GuiAutomationReport {
    let mut items = Vec::new();
    let result = status_inner(target, &mut items, runner).await;
    report(target, items, result)
}

pub async fn enable(target: &ComputeTarget, runner: &Runner) -> GuiAutomationReport {
    let mut items = Vec::new();
    let result = async {
        reconcile_app(target, &mut items, runner).await?;
        grant_accessibility_inner(target, &mut items, runner).await
    }
    .await;
    report(target, items, result)
}

pub async fn grant_accessibility(target: &ComputeTarget, runner: &Runner) -> GuiAutomationReport {
    let mut items = Vec::new();
    let result = grant_accessibility_inner(target, &mut items, runner).await;
    report(target, items, result)
}

pub async fn disable(target: &ComputeTarget, bundle: &str, runner: &Runner) -> GuiAutomationReport {
    let mut items = Vec::new();
    let result = disable_inner(target, bundle, &mut items, runner).await;
    report(target, items, result)
}
