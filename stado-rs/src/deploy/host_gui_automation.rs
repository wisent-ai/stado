//! Managed GUI automation lifecycle for a registry-owned macOS host.
//!
//! The host command owns the complete reusable path: install one pinned,
//! checksummed and signed CuaDriver release, install the reviewed Apple
//! challenge reader used by identity placement, grant both executables to the
//! login user's Accessibility database, report the resulting state, and remove
//! them. Every remote action is a fixed program invocation through
//! `host_channel`; source and sensitive values travel only on stdin.

use crate::deploy::{host_channel, CommandOutput, DeployError, Runner};
use crate::targets::ComputeTarget;

pub const CUA_DRIVER_VERSION: &str = "0.23.2";
pub const CUA_DRIVER_BUNDLE_ID: &str = "com.trycua.driver";
pub const CUA_DRIVER_APP: &str = "/Applications/CuaDriver.app";
const CUA_DRIVER_EXECUTABLE: &str = "/Applications/CuaDriver.app/Contents/MacOS/cua-driver";
pub const CUA_DRIVER_ARCHIVE_SHA256: &str =
    "9e521b16c8606896f20003f4d20ae62070a1cb3c8d33152d9d0593f62234fbb0";
pub const CUA_DRIVER_ARCHIVE_URL: &str = "https://github.com/trycua/cua/releases/download/\
    cua-driver-rs-v0.23.2/cua-driver-rs-0.23.2-darwin-universal.tar.gz";

pub const APPLE_CHALLENGE_HELPER_VERSION: &str = "2";
pub const APPLE_CHALLENGE_HELPER: &str = "/usr/local/libexec/stado-apple-challenge-capture";
const APPLE_CHALLENGE_HELPER_BUNDLE_ID: &str = "com.wisent.stado.apple-challenge-capture";
const APPLE_CHALLENGE_HELPER_SOURCE: &str =
    include_str!("../../scripts/capture-apple-challenge.swift");

pub(crate) struct AppleChallengeSession {
    user: String,
    uid: String,
}

const PLIST_BUDDY: &str = "/usr/libexec/PlistBuddy";
const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/\
    LaunchServices.framework/Versions/A/Support/lsregister";
const KICKSTART: &str = "/System/Library/CoreServices/RemoteManagement/ARDAgent.app/Contents/\
    Resources/kickstart";
const REMOTE_MANAGEMENT_PREFS: &str = "/Library/Preferences/com.apple.RemoteManagement";
const ACCESSIBILITY_SERVICE: &str = "kTCCServiceAccessibility";
const CUA_DRIVER_RUNTIME_LABEL: &str = "com.wisent.probierz-cua-driver";
const LEGACY_CUA_DRIVER_RUNTIME_LABEL: &str =
    "com.wisent.compute.service.com.wisent.probierz-cua-driver";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct HelperIdentity {
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
        ssh_target: target
            .ssh_connections()
            .next()
            .map_or_else(String::new, |(_, destination)| destination.to_string()),
        items,
        error: result.err().map(|error| error.0),
    }
}

fn require_target(target: &ComputeTarget) -> Result<(), DeployError> {
    if !target.has_ssh_connection() {
        return Err(DeployError(format!(
            "target {} has no SSH connection path in the registry",
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

async fn gui_user_id(
    target: &ComputeTarget,
    user: &str,
    runner: &Runner,
) -> Result<String, DeployError> {
    safe_identity(user, "GUI user")?;
    let uid = run(
        target,
        &["/usr/bin/id", "-u", user],
        "resolve GUI user id",
        runner,
    )
    .await?
    .stdout
    .trim()
    .to_string();
    if uid.is_empty() || !uid.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(DeployError(format!(
            "{} returned an invalid GUI user id: {uid}",
            target.name
        )));
    }
    Ok(uid)
}

async fn invoke_in_gui_session(
    target: &ComputeTarget,
    user: &str,
    uid: &str,
    words: &[&str],
    runner: &Runner,
) -> Result<CommandOutput, DeployError> {
    let mut command = Vec::with_capacity(words.len() + 10);
    command.extend(
        [
            "/usr/bin/sudo",
            "-n",
            "/bin/launchctl",
            "asuser",
            uid,
            "/usr/bin/sudo",
            "-n",
            "-u",
            user,
            "--",
        ]
        .into_iter()
        .map(str::to_string),
    );
    command.extend(words.iter().map(|word| (*word).to_string()));
    let command: Vec<&str> = command.iter().map(String::as_str).collect();
    host_channel::run_program(target, &command, runner).await
}

async fn invoke_as_gui_user(
    target: &ComputeTarget,
    user: &str,
    words: &[&str],
    runner: &Runner,
) -> Result<CommandOutput, DeployError> {
    let uid = gui_user_id(target, user, runner).await?;
    invoke_in_gui_session(target, user, &uid, words, runner).await
}

async fn run_in_gui_session(
    target: &ComputeTarget,
    user: &str,
    uid: &str,
    words: &[&str],
    what: &str,
    runner: &Runner,
) -> Result<CommandOutput, DeployError> {
    let output = invoke_in_gui_session(target, user, uid, words, runner).await?;
    if output.ok() {
        Ok(output)
    } else {
        Err(DeployError(format!(
            "{}: {what} failed for GUI user {user}: {}",
            target.name,
            output.detail().trim()
        )))
    }
}

async fn run_as_gui_user(
    target: &ComputeTarget,
    user: &str,
    words: &[&str],
    what: &str,
    runner: &Runner,
) -> Result<CommandOutput, DeployError> {
    let uid = gui_user_id(target, user, runner).await?;
    run_in_gui_session(target, user, &uid, words, what, runner).await
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
                "signed executable has no designated code requirement: {}",
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

fn apple_challenge_helper_path() -> &'static str {
    APPLE_CHALLENGE_HELPER
}

async fn helper_identity(
    target: &ComputeTarget,
    path: &str,
    runner: &Runner,
) -> Result<Option<HelperIdentity>, DeployError> {
    if !host_channel::remote_test(target, &format!("-f {}", super::shlex_quote(path)), runner)
        .await?
    {
        return Ok(None);
    }
    run(
        target,
        &["/usr/bin/codesign", "--verify", "--strict", path],
        "Apple challenge helper signature verification",
        runner,
    )
    .await?;
    let version_output = run(
        target,
        &[path, "--version"],
        "Apple challenge helper version read",
        runner,
    )
    .await?
    .stdout;
    let version = version_output
        .strip_prefix("stado-apple-challenge-capture ")
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    safe_identity(&version, "Apple challenge helper version")?;
    let requirement = designated_requirement(
        &run(
            target,
            &["/usr/bin/codesign", "-d", "-r-", path],
            "Apple challenge helper code requirement read",
            runner,
        )
        .await?,
    )?;
    Ok(Some(HelperIdentity {
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

async fn session_ready_for(
    target: &ComputeTarget,
    expected_user: &str,
    readiness_key: &str,
    runner: &Runner,
) -> Result<bool, DeployError> {
    let report = status(target, runner).await;
    if let Some(error) = report.error {
        return Err(DeployError(error));
    }
    let value = |key: &str| {
        report
            .items
            .iter()
            .find_map(|(name, value)| (name == key).then_some(value.as_str()))
    };
    Ok(value("console") == Some(expected_user)
        && value("accessibility-user") == Some(expected_user)
        && value(readiness_key) == Some("yes"))
}

/// Whether CuaDriver can drive this exact user's current GUI session.
pub async fn automated_session_ready_for(
    target: &ComputeTarget,
    expected_user: &str,
    runner: &Runner,
) -> Result<bool, DeployError> {
    session_ready_for(target, expected_user, "gui-ready", runner).await
}

/// Whether the signed helper can read this exact user's Apple challenge.
pub async fn apple_challenge_session_ready_for(
    target: &ComputeTarget,
    expected_user: &str,
    runner: &Runner,
) -> Result<bool, DeployError> {
    session_ready_for(target, expected_user, "apple-challenge-ready", runner).await
}

/// Exercise the exact signed AX client in the exact Aqua session without
/// scanning windows or opening a system prompt.
pub(crate) async fn preflight_apple_challenge(
    target: &ComputeTarget,
    expected_user: &str,
    runner: &Runner,
) -> Result<AppleChallengeSession, DeployError> {
    safe_identity(expected_user, "GUI user")?;
    if !apple_challenge_session_ready_for(target, expected_user, runner).await? {
        return Err(DeployError(format!(
            "{} does not have a ready Apple challenge session for {expected_user}",
            target.name
        )));
    }
    let user = login_user(target, runner).await?;
    if user != expected_user {
        return Err(DeployError(format!(
            "{} is ready for GUI user {user}, not {expected_user}",
            target.name
        )));
    }
    let uid = gui_user_id(target, &user, runner).await?;
    let output = invoke_in_gui_session(
        target,
        &user,
        &uid,
        &[apple_challenge_helper_path(), "--preflight"],
        runner,
    )
    .await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: the Apple challenge helper cannot use Accessibility in {user}'s Aqua session: {}",
            target.name,
            output.detail().trim()
        )));
    }
    let report: serde_json::Value =
        serde_json::from_str(output.stdout.trim()).map_err(|error| {
            DeployError(format!(
                "{}: Apple challenge preflight returned invalid JSON: {error}",
                target.name
            ))
        })?;
    if report.get("version").and_then(serde_json::Value::as_str)
        != Some(APPLE_CHALLENGE_HELPER_VERSION)
        || report.get("ok").and_then(serde_json::Value::as_bool) != Some(true)
        || report
            .get("accessibilityTrusted")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
    {
        return Err(DeployError(format!(
            "{}: Apple challenge preflight did not confirm Accessibility",
            target.name
        )));
    }
    Ok(AppleChallengeSession { user, uid })
}

/// Capture one Apple trusted-device code inside the verified GUI user's Aqua
/// session. The code exists only in an owner-only file on that host and in the
/// returned in-memory value; diagnostics never include it.
pub async fn capture_apple_challenge(
    target: &ComputeTarget,
    expected_user: &str,
    capture_id: &str,
    wait_seconds: u64,
    runner: &Runner,
) -> Result<String, DeployError> {
    safe_identity(expected_user, "GUI user")?;
    safe_identity(capture_id, "Apple challenge capture id")?;
    if !(1..=90).contains(&wait_seconds) {
        return Err(DeployError(
            "Apple challenge wait must be between 1 and 90 seconds".to_string(),
        ));
    }
    let session = preflight_apple_challenge(target, expected_user, runner).await?;
    let user = session.user;
    let uid = session.uid;

    let work = format!("/Users/{user}/.stado/work/apple-challenge");
    let output_file = format!("{work}/{capture_id}.code");
    run_in_gui_session(
        target,
        &user,
        &uid,
        &["/bin/mkdir", "-p", &work],
        "create Apple challenge work directory",
        runner,
    )
    .await?;
    run_in_gui_session(
        target,
        &user,
        &uid,
        &["/bin/chmod", "700", &work],
        "protect Apple challenge work directory",
        runner,
    )
    .await?;
    run_in_gui_session(
        target,
        &user,
        &uid,
        &["/bin/rm", "-f", &output_file],
        "remove stale Apple challenge file",
        runner,
    )
    .await?;

    let wait = wait_seconds.to_string();
    let capture = invoke_in_gui_session(
        target,
        &user,
        &uid,
        &[
            apple_challenge_helper_path(),
            "--output-file",
            &output_file,
            "--click-allow",
            "--click-done",
            "--wait-seconds",
            &wait,
        ],
        runner,
    )
    .await?;
    if !capture.ok() {
        let _ = run_in_gui_session(
            target,
            &user,
            &uid,
            &["/bin/rm", "-f", &output_file],
            "remove failed Apple challenge file",
            runner,
        )
        .await;
        return Err(DeployError(format!(
            "{} could not capture the Apple challenge: {}",
            target.name,
            capture.detail().trim()
        )));
    }

    let mut captured =
        invoke_in_gui_session(target, &user, &uid, &["/bin/cat", &output_file], runner).await?;
    let cleanup = run_in_gui_session(
        target,
        &user,
        &uid,
        &["/bin/rm", "-f", &output_file],
        "remove consumed Apple challenge file",
        runner,
    )
    .await;
    if !captured.ok() {
        return Err(DeployError(format!(
            "{} captured an Apple challenge but could not read its protected file",
            target.name
        )));
    }
    cleanup?;
    let code = captured.stdout.trim().to_string();
    captured.stdout.clear();
    if code.len() != 6 || !code.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(DeployError(format!(
            "{} returned an invalid Apple challenge",
            target.name
        )));
    }
    Ok(code)
}

/// The macOS users this host's registry identity bindings name, each with the identity
/// it holds.
///
/// `IdentityBinding::user` exists because these identities are per-user: an Apple
/// account signed into one macOS user does not make the Mac's other users trusted. A
/// notification for it is delivered into that user's session and is unreadable from
/// every other one.
fn declared_gui_bindings(target: &ComputeTarget) -> Vec<(String, String)> {
    let mut named: Vec<(String, String)> = Vec::new();
    for binding in &target.identities {
        let Some(user) = binding.user.as_deref() else {
            continue;
        };
        if user.is_empty() || named.iter().any(|(existing, _)| existing == user) {
            continue;
        }
        named.push((user.to_string(), binding.identity.clone()));
    }
    named
}

/// Is the session we are about to automate one the registry declares an identity in?
fn automates_declared_session(target: &ComputeTarget, user: &str) -> bool {
    let declared = declared_gui_bindings(target);
    declared.is_empty() || declared.iter().any(|(named, _)| named == user)
}

/// Refuse to enable automation for a session that holds none of the declared
/// identities.
///
/// Without this the resolution is silent and plausible: `login_user` answers with
/// whoever is at `/dev/console`, every step succeeds against that user, and `status`
/// ends with `gui-ready yes`. On charless-mac-mini on 2026-09-04 that sentence was
/// true about the `charles` session and useless about the fleet: the Apple account the
/// registry places there is signed into `controlyourai-relay`, whose prompts the
/// `charles` session cannot see. Enabling the wrong session is not partial progress
/// towards reading a code; it is a certainty of never reading one.
fn require_declared_session(target: &ComputeTarget, user: &str) -> Result<(), DeployError> {
    if automates_declared_session(target, user) {
        return Ok(());
    }
    let named = declared_gui_bindings(target)
        .iter()
        .map(|(named, identity)| format!("{named} holds {identity}"))
        .collect::<Vec<_>>()
        .join(", ");
    Err(DeployError(format!(
        "{}: the GUI session available for automation is {user}'s, and this host's \
         registry declares {named}. An identity signed into one macOS user is invisible \
         to the others, so automating {user} cannot read a prompt for it. Put the \
         declared user at the console, or correct the host's identity binding.",
        target.name
    )))
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

async fn reconcile_apple_challenge_helper(
    target: &ComputeTarget,
    items: &mut Vec<(String, String)>,
    runner: &Runner,
) -> Result<HelperIdentity, DeployError> {
    require_target(target)?;
    let path = apple_challenge_helper_path();
    if let Ok(Some(identity)) = helper_identity(target, path, runner).await {
        if identity.version == APPLE_CHALLENGE_HELPER_VERSION {
            items.push(("apple-challenge-helper".to_string(), "reused".to_string()));
            items.push((
                "apple-challenge-helper-version".to_string(),
                identity.version.clone(),
            ));
            return Ok(identity);
        }
    }

    let home = host_channel::remote_home(target, runner).await?;
    let cache = format!("{home}/.stado/cache/apple-challenge-helper");
    let source = format!("{cache}/capture.swift");
    let staged = format!("{cache}/stado-apple-challenge-capture.staged");
    run(
        target,
        &["/bin/mkdir", "-p", &cache],
        "create Apple challenge helper cache",
        runner,
    )
    .await?;
    remove_if_present(target, &source, false, runner).await?;
    remove_if_present(target, &staged, false, runner).await?;
    let output_argument = format!("of={source}");
    let source_write = host_channel::run_program_with_stdin(
        target,
        &["/bin/dd", &output_argument, "bs=65536"],
        APPLE_CHALLENGE_HELPER_SOURCE,
        runner,
    )
    .await?;
    if !source_write.ok() {
        return Err(DeployError(format!(
            "{}: writing the Apple challenge helper source failed: {}",
            target.name,
            source_write.detail().trim()
        )));
    }
    run(
        target,
        &["/bin/chmod", "600", &source],
        "protect Apple challenge helper source",
        runner,
    )
    .await?;
    run(
        target,
        &["/usr/bin/xcrun", "swiftc", "-O", &source, "-o", &staged],
        "compile Apple challenge helper",
        runner,
    )
    .await?;
    run(
        target,
        &[
            "/usr/bin/codesign",
            "--force",
            "--sign",
            "-",
            "--identifier",
            APPLE_CHALLENGE_HELPER_BUNDLE_ID,
            &staged,
        ],
        "sign Apple challenge helper",
        runner,
    )
    .await?;
    run_sudo(
        target,
        &["/bin/mkdir", "-p", "/usr/local/libexec"],
        "create the system helper directory",
        runner,
    )
    .await?;
    run_sudo(
        target,
        &[
            "/usr/bin/install",
            "-o",
            "root",
            "-g",
            "wheel",
            "-m",
            "755",
            &staged,
            path,
        ],
        "install Apple challenge helper",
        runner,
    )
    .await?;
    remove_if_present(target, &source, false, runner).await?;
    remove_if_present(target, &staged, false, runner).await?;

    let identity = helper_identity(target, path, runner)
        .await?
        .ok_or_else(|| DeployError("Apple challenge helper was not installed".to_string()))?;
    if identity.version != APPLE_CHALLENGE_HELPER_VERSION {
        return Err(DeployError(format!(
            "Apple challenge helper is version {}, expected {}",
            identity.version, APPLE_CHALLENGE_HELPER_VERSION
        )));
    }
    items.push((
        "apple-challenge-helper".to_string(),
        "installed".to_string(),
    ));
    items.push((
        "apple-challenge-helper-version".to_string(),
        identity.version.clone(),
    ));
    Ok(identity)
}

async fn code_requirement_hex(
    target: &ComputeTarget,
    home: &str,
    name: &str,
    requirement: &str,
    runner: &Runner,
) -> Result<String, DeployError> {
    safe_identity(name, "code requirement name")?;
    let cache = format!("{home}/.stado/cache/gui-automation");
    let requirement_file = format!("{cache}/{name}.csreq");
    run(
        target,
        &["/bin/mkdir", "-p", &cache],
        "create GUI automation cache",
        runner,
    )
    .await?;
    remove_if_present(target, &requirement_file, false, runner).await?;
    let requirement_argument = format!("={requirement}");
    run(
        target,
        &[
            "/usr/bin/csreq",
            "-r",
            &requirement_argument,
            "-b",
            &requirement_file,
        ],
        "compile GUI automation code requirement",
        runner,
    )
    .await?;
    let encoded = run(
        target,
        &["/usr/bin/xxd", "-p", &requirement_file],
        "encode GUI automation code requirement",
        runner,
    )
    .await?
    .stdout
    .split_whitespace()
    .collect::<String>();
    remove_if_present(target, &requirement_file, false, runner).await?;
    if encoded.is_empty() || !encoded.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(DeployError(format!(
            "compiled {name} code requirement is invalid"
        )));
    }
    Ok(encoded)
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
        // The host channel has a bounded command window. Keep a verified
        // version-scoped partial and resume it on the next reconciliation;
        // deleting it first makes every slow GitHub download restart at byte
        // zero and therefore guarantees the same timeout forever.
        run(
            target,
            &[
                "/usr/bin/curl",
                "-fL",
                "--retry",
                "3",
                "--continue-at",
                "-",
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
fn kcpassword_hex(password: &str) -> Result<String, DeployError> {
    if password.is_empty() {
        return Err(DeployError("host account password is empty".to_string()));
    }
    const KEY: [u8; 11] = [
        0x7d, 0x89, 0x52, 0x23, 0xd2, 0xbc, 0xdd, 0xea, 0xa3, 0xb9, 0x1f,
    ];
    let mut bytes = password.as_bytes().to_vec();
    bytes.push(0);
    while !bytes.len().is_multiple_of(KEY.len()) {
        bytes.push(0);
    }
    Ok(bytes
        .iter()
        .enumerate()
        .map(|(index, byte)| format!("{:02x}", byte ^ KEY[index % KEY.len()]))
        .collect())
}

async fn reconcile_autologin(
    target: &ComputeTarget,
    password: &str,
    items: &mut Vec<(String, String)>,
    runner: &Runner,
) -> Result<(), DeployError> {
    let user = login_user(target, runner).await?;
    let encoded = kcpassword_hex(password)?;
    let staged = "/etc/kcpassword.stado";
    let written = host_channel::run_program_with_stdin(
        target,
        &[
            "/usr/bin/sudo",
            "-n",
            "/usr/bin/xxd",
            "-r",
            "-p",
            "/dev/stdin",
            staged,
        ],
        &encoded,
        runner,
    )
    .await?;
    if !written.ok() {
        return Err(DeployError(format!(
            "{}: writing the staged autologin credential failed: {}",
            target.name,
            written.detail().trim()
        )));
    }
    run_sudo(
        target,
        &["/usr/sbin/chown", "root:wheel", staged],
        "set staged autologin credential owner",
        runner,
    )
    .await?;
    run_sudo(
        target,
        &["/bin/chmod", "600", staged],
        "set staged autologin credential mode",
        runner,
    )
    .await?;
    run_sudo(
        target,
        &["/bin/test", "-s", staged],
        "verify the staged autologin credential",
        runner,
    )
    .await?;
    run_sudo(
        target,
        &["/bin/mv", "-f", staged, "/etc/kcpassword"],
        "install the autologin credential",
        runner,
    )
    .await?;
    run_sudo(
        target,
        &[
            "/usr/bin/defaults",
            "write",
            "/Library/Preferences/com.apple.loginwindow",
            "autoLoginUser",
            "-string",
            &user,
        ],
        "configure the persistent GUI login user",
        runner,
    )
    .await?;
    let configured = run_sudo(
        target,
        &[
            "/usr/bin/defaults",
            "read",
            "/Library/Preferences/com.apple.loginwindow",
            "autoLoginUser",
        ],
        "verify the persistent GUI login user",
        runner,
    )
    .await?
    .stdout;
    if configured.trim() != user {
        return Err(DeployError(
            "the persistent GUI login user was not read back".to_string(),
        ));
    }
    items.push(("autologin".to_string(), user));
    items.push(("kcpassword".to_string(), "present".to_string()));
    Ok(())
}

async fn grant_accessibility_inner(
    target: &ComputeTarget,
    items: &mut Vec<(String, String)>,
    apple_only: bool,
    runner: &Runner,
) -> Result<(), DeployError> {
    require_target(target)?;
    let identity = if apple_only {
        None
    } else {
        let identity = app_identity(target, CUA_DRIVER_APP, runner)
            .await?
            .ok_or_else(|| DeployError("CuaDriver.app is not installed".to_string()))?;
        if identity.bundle != CUA_DRIVER_BUNDLE_ID {
            return Err(DeployError(format!(
                "CuaDriver.app has bundle id {}, expected {}",
                identity.bundle, CUA_DRIVER_BUNDLE_ID
            )));
        }
        Some(identity)
    };
    let helper = helper_identity(target, apple_challenge_helper_path(), runner)
        .await?
        .ok_or_else(|| DeployError("Apple challenge helper is not installed".to_string()))?;
    if helper.version != APPLE_CHALLENGE_HELPER_VERSION {
        return Err(DeployError(format!(
            "Apple challenge helper is version {}, expected {}",
            helper.version, APPLE_CHALLENGE_HELPER_VERSION
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

    let command_home = host_channel::remote_home(target, runner).await?;
    let cua_requirement = if let Some(identity) = &identity {
        Some(
            code_requirement_hex(
                target,
                &command_home,
                "cua-driver",
                &identity.requirement,
                runner,
            )
            .await?,
        )
    } else {
        None
    };
    let helper_requirement = code_requirement_hex(
        target,
        &command_home,
        "apple-challenge",
        &helper.requirement,
        runner,
    )
    .await?;

    let backup_dir = format!("{home}/.stado/backups");
    let backup = format!("{backup_dir}/TCC.db.before-stado-accessibility");
    run_as_gui_user(
        target,
        &user,
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

    // CuaDriver is launched directly and through LaunchServices; the Apple
    // helper is a separate signed executable run in that same Aqua session.
    // TCC identifies all three responsibility chains separately.
    let insert = |client: &str, client_type: u8, requirement: &str| {
        format!(
            "INSERT INTO access (service, client, client_type, auth_value, auth_reason, \
             auth_version, csreq, policy_id, indirect_object_identifier_type, \
             indirect_object_identifier, indirect_object_code_identity, flags, last_modified) \
             VALUES ('{ACCESSIBILITY_SERVICE}', '{client}', {client_type}, 2, 3, 1, \
             X'{requirement}', NULL, 0, 'UNUSED', NULL, 0, strftime('%s','now'));"
        )
    };
    let (clients, inserts, expected_count) =
        if let (Some(identity), Some(requirement)) = (&identity, &cua_requirement) {
            (
                format!(
                    "((client = '{}' AND client_type = 0) \
                     OR (client = '{CUA_DRIVER_EXECUTABLE}' AND client_type = 1) \
                     OR (client = '{}' AND client_type = 1))",
                    identity.bundle,
                    apple_challenge_helper_path(),
                ),
                format!(
                    "{} {} {}",
                    insert(&identity.bundle, 0, requirement),
                    insert(CUA_DRIVER_EXECUTABLE, 1, requirement),
                    insert(apple_challenge_helper_path(), 1, &helper_requirement),
                ),
                "3",
            )
        } else {
            (
                format!(
                    "(client = '{}' AND client_type = 1)",
                    apple_challenge_helper_path(),
                ),
                insert(apple_challenge_helper_path(), 1, &helper_requirement),
                "1",
            )
        };
    let sql = format!(
        "BEGIN IMMEDIATE; DELETE FROM access WHERE service = '{ACCESSIBILITY_SERVICE}' \
         AND {clients}; {inserts} COMMIT;",
    );
    run_sudo(
        target,
        &["/usr/bin/sqlite3", &database, &sql],
        "grant GUI automation Accessibility",
        runner,
    )
    .await?;
    let verify_sql = format!(
        "SELECT COUNT(*) FROM access WHERE service = '{ACCESSIBILITY_SERVICE}' \
         AND auth_value = 2 AND {clients};",
    );
    let granted = run_sudo(
        target,
        &["/usr/bin/sqlite3", &database, &verify_sql],
        "verify GUI automation Accessibility",
        runner,
    )
    .await?
    .stdout;
    if granted.trim() != expected_count {
        return Err(DeployError(
            if apple_only {
                "the Apple challenge Accessibility grant was not read back"
            } else {
                "the CuaDriver and Apple challenge Accessibility grants were not read back"
            }
            .to_string(),
        ));
    }
    if let Some(identity) = identity {
        items.push(("accessibility".to_string(), "granted".to_string()));
        items.push(("accessibility-client".to_string(), identity.bundle));
    }
    if apple_only {
        preflight_apple_challenge(target, &user, runner).await?;
        items.push(("apple-challenge-ready".to_string(), "yes".to_string()));
    }
    items.push((
        "apple-challenge-accessibility".to_string(),
        "granted".to_string(),
    ));
    items.push(("accessibility-user".to_string(), user));
    items.push(("accessibility-backup".to_string(), backup));
    Ok(())
}

async fn reconcile_runtime(
    target: &ComputeTarget,
    items: &mut Vec<(String, String)>,
    runner: &Runner,
) -> Result<(), DeployError> {
    require_target(target)?;
    let user = login_user(target, runner).await?;
    let uid = gui_user_id(target, &user, runner).await?;
    let home = format!("/Users/{user}");
    let launch_agents = format!("{home}/Library/LaunchAgents");
    let caches = format!("{home}/Library/Caches/cua-driver");
    let logs = format!("{home}/.stado/logs");
    let plist = format!("{launch_agents}/{CUA_DRIVER_RUNTIME_LABEL}.plist");
    let staged = format!("{plist}.stado");
    let socket = format!("{caches}/probierz.sock");
    let stdout = format!("{logs}/probierz-cua-driver.out");
    let stderr = format!("{logs}/probierz-cua-driver.err");
    // LaunchServices supplies the WindowServer/Aqua responsibility chain that
    // AppKit (including NSPasteboard and Accessibility) requires. Starting the
    // Mach-O directly from launchd leaves those APIs unavailable.
    let arguments = serde_json::to_string(&[
        "/usr/bin/open",
        "-n",
        "-g",
        "-a",
        "CuaDriver",
        "--args",
        "serve",
        "--socket",
        socket.as_str(),
        "--no-permissions-gate",
    ])
    .map_err(|error| DeployError(format!("cannot encode CuaDriver arguments: {error}")))?;

    run_as_gui_user(
        target,
        &user,
        &["/bin/mkdir", "-p", &launch_agents, &caches, &logs],
        "create CuaDriver runtime directories",
        runner,
    )
    .await?;
    run_as_gui_user(
        target,
        &user,
        &["/bin/rm", "-f", &staged],
        "remove stale CuaDriver LaunchAgent staging file",
        runner,
    )
    .await?;
    run_as_gui_user(
        target,
        &user,
        &["/usr/bin/plutil", "-create", "xml1", &staged],
        "create CuaDriver LaunchAgent",
        runner,
    )
    .await?;
    for (key, value) in [
        ("Label", CUA_DRIVER_RUNTIME_LABEL),
        ("ProgramArguments", arguments.as_str()),
        ("LimitLoadToSessionType", "Aqua"),
        ("StandardOutPath", stdout.as_str()),
        ("StandardErrorPath", stderr.as_str()),
    ] {
        let kind = if key == "ProgramArguments" {
            "-json"
        } else {
            "-string"
        };
        run_as_gui_user(
            target,
            &user,
            &["/usr/bin/plutil", "-insert", key, kind, value, &staged],
            "write CuaDriver LaunchAgent",
            runner,
        )
        .await?;
    }
    run_as_gui_user(
        target,
        &user,
        &[
            "/usr/bin/plutil",
            "-insert",
            "RunAtLoad",
            "-bool",
            "true",
            &staged,
        ],
        "write CuaDriver LaunchAgent",
        runner,
    )
    .await?;
    run_as_gui_user(
        target,
        &user,
        &["/usr/bin/plutil", "-lint", &staged],
        "validate CuaDriver LaunchAgent",
        runner,
    )
    .await?;

    let qualified = format!("gui/{uid}/{CUA_DRIVER_RUNTIME_LABEL}");
    let definition_matches = invoke_as_gui_user(
        target,
        &user,
        &["/usr/bin/cmp", "-s", &staged, &plist],
        runner,
    )
    .await?
    .ok();
    let runtime_loaded = invoke_as_gui_user(
        target,
        &user,
        &["/bin/launchctl", "print", &qualified],
        runner,
    )
    .await?
    .ok();
    let socket_ready = invoke_as_gui_user(target, &user, &["/bin/test", "-S", &socket], runner)
        .await?
        .ok();
    if definition_matches && runtime_loaded && socket_ready {
        run_as_gui_user(
            target,
            &user,
            &["/bin/rm", "-f", &staged],
            "remove matched CuaDriver LaunchAgent staging file",
            runner,
        )
        .await?;
        items.push(("cua-driver-runtime".to_string(), "running".to_string()));
        items.push(("cua-driver-socket".to_string(), socket));
        return Ok(());
    }

    for label in [CUA_DRIVER_RUNTIME_LABEL, LEGACY_CUA_DRIVER_RUNTIME_LABEL] {
        let qualified = format!("gui/{uid}/{label}");
        let _ = invoke_as_gui_user(
            target,
            &user,
            &["/bin/launchctl", "bootout", &qualified],
            runner,
        )
        .await?;
    }
    run_as_gui_user(
        target,
        &user,
        &["/bin/rm", "-f", &socket],
        "remove stale CuaDriver socket",
        runner,
    )
    .await?;
    run_as_gui_user(
        target,
        &user,
        &["/bin/mv", "-f", &staged, &plist],
        "install CuaDriver LaunchAgent",
        runner,
    )
    .await?;
    let domain = format!("gui/{uid}");
    run_as_gui_user(
        target,
        &user,
        &["/bin/launchctl", "bootstrap", &domain, &plist],
        "bootstrap CuaDriver LaunchAgent",
        runner,
    )
    .await?;
    let qualified = format!("{domain}/{CUA_DRIVER_RUNTIME_LABEL}");
    run_as_gui_user(
        target,
        &user,
        &["/bin/launchctl", "kickstart", "-k", &qualified],
        "start CuaDriver LaunchAgent",
        runner,
    )
    .await?;

    let mut socket_ready = false;
    for _ in 0..20 {
        if invoke_as_gui_user(target, &user, &["/bin/test", "-S", &socket], runner)
            .await?
            .ok()
        {
            socket_ready = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    if !socket_ready {
        return Err(DeployError(format!(
            "CuaDriver LaunchAgent started but did not create {socket}"
        )));
    }
    items.push(("cua-driver-runtime".to_string(), "running".to_string()));
    items.push(("cua-driver-socket".to_string(), socket));
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
    items.push(("console".to_string(), console.clone()));

    let user = login_user(target, runner).await?;
    let database = format!("/Users/{user}/Library/Application Support/com.apple.TCC/TCC.db");
    let identity = app_identity(target, CUA_DRIVER_APP, runner).await?;
    if let Some(identity) = &identity {
        items.push(("cua-driver-app".to_string(), "present".to_string()));
        items.push(("cua-driver-version".to_string(), identity.version.clone()));
        items.push(("cua-driver-client".to_string(), identity.bundle.clone()));
    } else {
        items.push(("cua-driver-app".to_string(), "absent".to_string()));
    }

    let helper = helper_identity(target, apple_challenge_helper_path(), runner).await?;
    if let Some(helper) = &helper {
        items.push(("apple-challenge-helper".to_string(), "present".to_string()));
        items.push((
            "apple-challenge-helper-version".to_string(),
            helper.version.clone(),
        ));
    } else {
        items.push(("apple-challenge-helper".to_string(), "absent".to_string()));
    }

    let accessibility = if let Some(identity) = &identity {
        let query = format!(
            "SELECT COUNT(*) FROM access WHERE service = '{ACCESSIBILITY_SERVICE}' \
             AND auth_value = 2 AND ((client = '{}' AND client_type = 0) \
             OR (client = '{CUA_DRIVER_EXECUTABLE}' AND client_type = 1));",
            identity.bundle
        );
        let value = optional_sudo(target, &["/usr/bin/sqlite3", &database, &query], runner)
            .await?
            .unwrap_or_default();
        match value.trim() {
            "2" => "granted".to_string(),
            "0" | "1" | "" => "not-set".to_string(),
            other => format!("refused:{other}"),
        }
    } else {
        "app-missing".to_string()
    };
    items.push(("accessibility".to_string(), accessibility.clone()));

    let challenge_accessibility = if helper.is_some() {
        let query = format!(
            "SELECT COUNT(*) FROM access WHERE service = '{ACCESSIBILITY_SERVICE}' \
             AND auth_value = 2 AND client = '{}' AND client_type = 1;",
            apple_challenge_helper_path()
        );
        let value = optional_sudo(target, &["/usr/bin/sqlite3", &database, &query], runner)
            .await?
            .unwrap_or_default();
        match value.trim() {
            "1" => "granted".to_string(),
            "0" | "" => "not-set".to_string(),
            other => format!("refused:{other}"),
        }
    } else {
        "helper-missing".to_string()
    };
    items.push((
        "apple-challenge-accessibility".to_string(),
        challenge_accessibility.clone(),
    ));
    items.push(("accessibility-user".to_string(), user.clone()));

    let uid = gui_user_id(target, &user, runner).await?;
    let qualified = format!("gui/{uid}/{CUA_DRIVER_RUNTIME_LABEL}");
    let runtime = if invoke_as_gui_user(
        target,
        &user,
        &["/bin/launchctl", "print", &qualified],
        runner,
    )
    .await?
    .ok()
    {
        "running"
    } else {
        "absent"
    };
    let socket = format!("/Users/{user}/Library/Caches/cua-driver/probierz.sock");
    let socket_ready = invoke_as_gui_user(target, &user, &["/bin/test", "-S", &socket], runner)
        .await?
        .ok();
    items.push(("cua-driver-runtime".to_string(), runtime.to_string()));
    items.push((
        "cua-driver-socket".to_string(),
        if socket_ready { "ready" } else { "absent" }.to_string(),
    ));

    let console_ready = !matches!(console.as_str(), "" | "root" | "loginwindow" | "unknown");
    // Whose session this is belongs in the readiness answer, not beside it. A host can
    // hold a driver, grants and a live socket in one user's session while the identity
    // the fleet placed here lives in another.
    for (named, held) in declared_gui_bindings(target) {
        items.push((format!("identity-user:{held}"), named));
    }
    let declared_session = automates_declared_session(target, &user);
    items.push((
        "automated-session-declared".to_string(),
        if declared_session { "yes" } else { "no" }.to_string(),
    ));
    let gui_ready = console_ready
        && declared_session
        && accessibility == "granted"
        && runtime == "running"
        && socket_ready;
    items.push((
        "gui-ready".to_string(),
        if gui_ready { "yes" } else { "no" }.to_string(),
    ));
    let challenge_ready = console_ready
        && declared_session
        && helper
            .as_ref()
            .is_some_and(|value| value.version == APPLE_CHALLENGE_HELPER_VERSION)
        && challenge_accessibility == "granted";
    items.push((
        "apple-challenge-ready".to_string(),
        if challenge_ready { "yes" } else { "no" }.to_string(),
    ));
    Ok(())
}

async fn disable_inner(
    target: &ComputeTarget,
    bundle: &str,
    items: &mut Vec<(String, String)>,
    runner: &Runner,
) -> Result<(), DeployError> {
    require_target(target)?;
    let user = login_user(target, runner).await?;
    let uid = gui_user_id(target, &user, runner).await?;
    let home = format!("/Users/{user}");
    let command_home = host_channel::remote_home(target, runner).await?;

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
    }
    let database = format!("{home}/Library/Application Support/com.apple.TCC/TCC.db");
    let bundle_clause = if bundle.is_empty() {
        String::new()
    } else {
        format!(" OR (client = '{bundle}' AND client_type = 0)")
    };
    let sql = format!(
        "DELETE FROM access WHERE service = '{ACCESSIBILITY_SERVICE}' AND \
         ((client = '{CUA_DRIVER_EXECUTABLE}' AND client_type = 1) \
         OR (client = '{}' AND client_type = 1){bundle_clause});",
        apple_challenge_helper_path()
    );
    run_sudo(
        target,
        &["/usr/bin/sqlite3", &database, &sql],
        "revoke GUI automation Accessibility",
        runner,
    )
    .await?;
    items.push((
        "tcc-revoked".to_string(),
        "CuaDriver and Apple challenge helper".to_string(),
    ));

    for label in [CUA_DRIVER_RUNTIME_LABEL, LEGACY_CUA_DRIVER_RUNTIME_LABEL] {
        let qualified = format!("gui/{uid}/{label}");
        let _ = invoke_as_gui_user(
            target,
            &user,
            &["/bin/launchctl", "bootout", &qualified],
            runner,
        )
        .await?;
    }
    for path in [
        format!("{home}/Library/LaunchAgents/{CUA_DRIVER_RUNTIME_LABEL}.plist"),
        format!("{home}/Library/Caches/cua-driver/probierz.sock"),
        format!("{home}/.local/bin/cua-driver"),
        format!("{home}/.stado/cache/cua-driver"),
        format!("{home}/.stado/cache/apple-challenge-helper"),
        format!("{home}/.stado/cache/gui-automation"),
    ] {
        run_as_gui_user(
            target,
            &user,
            &["/bin/rm", "-rf", &path],
            "remove GUI automation user state",
            runner,
        )
        .await?;
    }
    items.push(("cua-driver-runtime".to_string(), "removed".to_string()));

    remove_if_present(target, CUA_DRIVER_APP, true, runner).await?;
    remove_if_present(target, apple_challenge_helper_path(), true, runner).await?;
    if command_home != home {
        for path in [
            format!("{command_home}/.stado/cache/cua-driver"),
            format!("{command_home}/.stado/cache/apple-challenge-helper"),
            format!("{command_home}/.stado/cache/gui-automation"),
        ] {
            remove_if_present(target, &path, false, runner).await?;
        }
    }
    items.push(("cua-driver-app".to_string(), "removed".to_string()));
    items.push(("apple-challenge-helper".to_string(), "removed".to_string()));
    Ok(())
}

pub async fn status(target: &ComputeTarget, runner: &Runner) -> GuiAutomationReport {
    let mut items = Vec::new();
    let result = status_inner(target, &mut items, runner).await;
    report(target, items, result)
}

pub async fn enable(
    target: &ComputeTarget,
    password: &str,
    runner: &Runner,
) -> GuiAutomationReport {
    let mut items = Vec::new();
    let result = async {
        // Resolved and checked before the first change, not after: autologin, a
        // kcpassword file, a TCC grant and a launchd job all name one user, and the
        // wrong user is four writes to undo.
        let user = login_user(target, runner).await?;
        require_declared_session(target, &user)?;
        items.push(("automated-session".to_string(), user));
        reconcile_app(target, &mut items, runner).await?;
        reconcile_apple_challenge_helper(target, &mut items, runner).await?;
        reconcile_autologin(target, password, &mut items, runner).await?;
        grant_accessibility_inner(target, &mut items, false, runner).await?;
        reconcile_runtime(target, &mut items, runner).await
    }
    .await;
    report(target, items, result)
}

pub async fn grant_accessibility(
    target: &ComputeTarget,
    apple_only: bool,
    runner: &Runner,
) -> GuiAutomationReport {
    let mut items = Vec::new();
    let result = async {
        let user = login_user(target, runner).await?;
        require_declared_session(target, &user)?;
        items.push(("automated-session".to_string(), user));
        reconcile_apple_challenge_helper(target, &mut items, runner).await?;
        grant_accessibility_inner(target, &mut items, apple_only, runner).await?;
        if !apple_only {
            reconcile_runtime(target, &mut items, runner).await?;
        }
        Ok(())
    }
    .await;
    report(target, items, result)
}

pub async fn disable(target: &ComputeTarget, bundle: &str, runner: &Runner) -> GuiAutomationReport {
    let mut items = Vec::new();
    let result = disable_inner(target, bundle, &mut items, runner).await;
    report(target, items, result)
}
