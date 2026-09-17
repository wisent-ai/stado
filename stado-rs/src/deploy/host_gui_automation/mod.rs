//! Managed GUI automation lifecycle for a registry-owned macOS host.
//!
//! The host command owns the complete reusable path: install one pinned,
//! checksummed and signed CuaDriver release, install the reviewed Apple
//! challenge reader used by identity placement, grant both executables to the
//! login user's Accessibility database, report the resulting state, and remove
//! them. Every remote action is a fixed program invocation through
//! `host_channel`; source and sensitive values travel only on stdin.

use crate::deploy::{host_channel, shlex_quote, CommandOutput, DeployError, Runner};
use crate::targets::ComputeTarget;

mod challenge;
mod driver;
mod session;
mod state;

use challenge::*;
use driver::*;
use session::*;
use state::*;

pub use challenge::capture_apple_challenge;
pub(crate) use challenge::preflight_apple_challenge;
pub use driver::{apple_challenge_session_ready_for, automated_session_ready_for};

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
const APPLE_CHALLENGE_HELPER_SOURCE: &str = concat!(
    include_str!("../../host_payloads/capture_apple_challenge/01-accessibility-capture.swift"),
    include_str!("../../host_payloads/capture_apple_challenge/02-prompt-resolution.swift"),
);

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

pub async fn status(
    target: &ComputeTarget,
    password: Option<&str>,
    runner: &Runner,
) -> GuiAutomationReport {
    let mut items = Vec::new();
    let result = async {
        require_target(target)?;
        host_channel::with_session(
            target,
            runner,
            status_inner(target, &mut items, password, runner),
        )
        .await
    }
    .await;
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
        reconcile_apple_challenge_helper(target, &mut items, Some(password), runner).await?;
        reconcile_autologin(target, password, &mut items, runner).await?;
        grant_accessibility_inner(target, &mut items, false, Some(password), runner).await?;
        reconcile_runtime(target, &mut items, runner).await
    }
    .await;
    report(target, items, result)
}

pub async fn grant_accessibility(
    target: &ComputeTarget,
    apple_only: bool,
    password: Option<&str>,
    runner: &Runner,
) -> GuiAutomationReport {
    let mut items = Vec::new();
    let result = async {
        require_target(target)?;
        host_channel::with_session(target, runner, async {
            let user = login_user(target, runner).await?;
            require_declared_session(target, &user)?;
            items.push(("automated-session".to_string(), user));
            reconcile_apple_challenge_helper(target, &mut items, password, runner).await?;
            grant_accessibility_inner(target, &mut items, apple_only, password, runner).await?;
            if !apple_only {
                reconcile_runtime(target, &mut items, runner).await?;
            }
            Ok(())
        })
        .await
    }
    .await;
    report(target, items, result)
}

pub async fn disable(target: &ComputeTarget, bundle: &str, runner: &Runner) -> GuiAutomationReport {
    let mut items = Vec::new();
    let result = disable_inner(target, bundle, &mut items, runner).await;
    report(target, items, result)
}
