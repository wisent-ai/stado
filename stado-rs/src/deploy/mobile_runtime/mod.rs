//! Verify and repair the mobile automation runtime a host declares it needs.
//!
//! NO Python original. This module exists because of what stopped four Spis
//! crawl families on 2026-09-03: neither `appium` nor `adb` is installed on
//! either macOS host, so the iOS and Android capture placements have no
//! driver to open an application with and no bridge to reach a device
//! through.
//!
//! The probe half already existed and the repair half did not.
//! [`super::host_exec`] approved `appium --version`,
//! `appium driver list --installed`, `which adb` and `adb devices -l` on
//! 2026-09-03 precisely so a crawl coordinator could ask a placement host
//! whether it can run before submitting a job. Nothing could act on the
//! answer: `host software` reports what a host runs and stops there, and the
//! only remaining route was an `npm install -g appium` typed into somebody's
//! terminal — the unrepeatable, unauditable change
//! [`super::weles_browser_runtime`] was written to replace for Playwright.
//! This is the same shape for the same reason.
//!
//! Four properties are deliberate:
//!
//! 1. **The requirement is declared, never hardcoded.** It is read from
//!    [`crate::targets::ComputeTarget::mobile_runtime`], so a host that is
//!    not a mobile placement declares nothing and is not judged, and the
//!    version this verifies is the version the fleet asked for rather than a
//!    constant in whatever checkout an operator happens to run.
//! 2. **Verification probes the paths [`super::host_exec`] already names.**
//!    Not `PATH`: a non-interactive ssh session on a Homebrew host has none
//!    of these directories on it, which is why `which adb` and an absolute
//!    probe answer different questions and why the allowlist carries both.
//!    Sharing one candidate order is what keeps
//!    `stado host exec TARGET -- appium --version` and this command from
//!    naming different binaries on the same machine.
//! 3. **The host's own answer decides, never the installer's exit code.**
//!    Repair re-verifies, for the reason
//!    [`super::weles_browser_runtime`] does: an install that prints success
//!    and leaves the program absent is the failure this exists to catch.
//! 4. **Repair installs into the login user's home and nothing else.** The
//!    npm prefix is `~/.npm-global`, the first candidate the allowlist names;
//!    platform-tools land under `~/Library/Android/sdk/platform-tools`, the
//!    first candidate for `adb`. Nothing is written outside `$HOME`, no
//!    installer is run under `sudo`, and no service is touched.
//!
//! **Where the bytes come from, and where they do not.** A Stado product
//! reaches a host through the fleet object API — `host_release` fetches
//! `stado://releases/...` through `/api/release/object` and verifies the
//! archive against the canonical release manifest by digest. None of that
//! applies here, and saying so is part of the report: Appium is an npm
//! package and platform-tools is Google's archive, so the host fetches them
//! from `registry.npmjs.org` and `dl.google.com` over its own egress. That
//! is the same trust boundary `weles_browser_runtime` already crosses when
//! `playwright install` pulls from Playwright's CDN, and it is NOT the
//! release channel: no Stado digest covers these bytes, and the only
//! integrity statement available is the version readback this module takes
//! afterwards.
//!
//! The parts, in the order a caller meets them: `report` is the vocabulary a
//! component's state is stated in, `paths` renders the one candidate table
//! this module and [`super::host_exec`] share, `inventory` answers which host
//! may take which capture family, and `session` is the round trip that reads
//! a host and the install that repairs it.

mod inventory;
mod paths;
mod report;
mod session;

pub use inventory::{family_driver, placements, Placement, CAPABILITY_ID, FAMILIES};
pub use paths::{candidate_words, ANDROID_SDK_ROOT, NPM_PREFIX};
pub use report::{
    requirement, ComponentState, RuntimeReport, COMPONENT_DRIFTED, COMPONENT_MISSING,
    COMPONENT_PRESENT, COMPONENT_UNDECLARED_INCOMPATIBLE, COMPONENT_UNKNOWN, OK_STATUS,
    RUNTIME_COMPLETE, RUNTIME_INCOMPLETE, RUNTIME_UNKNOWN,
};
pub use session::{incompatible_drivers, installed_driver_version, repair, verify};
