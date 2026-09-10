//! Verify and repair the browser runtime a Weles host declares it needs.
//!
//! NO Python original. This module exists because of what stopped the first
//! real `generic_browser_task` on charless-mac-mini on 2026-08-30:
//!
//! ```text
//! browserContext.newPage: Executable doesn't exist at
//!   /Users/charles/Library/Caches/ms-playwright/ffmpeg-1011/ffmpeg-mac
//! ...Video rendering requires ffmpeg binary...
//! ```
//!
//! Three browser runs had already failed that way earlier the same day. The
//! worker records its sessions, and the recordings are the evidence Weles
//! exists to keep, so `newPage` dies before any navigation and every browser
//! task on the host fails. Turning recording off would trade the product's
//! own evidence for a green run; completing the runtime is the repair.
//!
//! Nothing in Stado installed or repaired anything on a host: the software report
//! says what a host runs and stops there. So the alternative to this module
//! was an `npx playwright install` typed into somebody's terminal — an
//! unrepeatable change nobody can audit and nobody can apply to the next host.
//!
//! Two properties are deliberate:
//!
//! 1. **The requirement is read from the release, never hardcoded.** Playwright
//!    pins an exact revision per component in
//!    `node_modules/playwright-core/browsers.json` inside the installed Weles
//!    release, and the cache directory name is `<name>-<revision>`. A constant
//!    here would drift from the release the host actually runs and would then
//!    verify the wrong path — which is the same class of defect as a marker
//!    naming a port nothing serves. The file is fetched byte-exact through
//!    [`super::service_file_fetch`] because a clamped or sanitized read of a
//!    JSON document is not the document.
//! 2. **Requirements and page readiness are separate facts.** `--component`
//!    selects the components this invocation requires and `ffmpeg` remains the
//!    default because recording was the incident this command first repaired.
//!    Independently, the report checks whether any Chromium, Firefox, or WebKit
//!    engine is present. Satisfying a recording-only requirement therefore
//!    cannot report a host with no browser as complete. Repair stays opt-in per
//!    component and never downloads an engine unless the operator names it.

mod probe;
mod remote;
mod report;
mod requirement;

pub use probe::{repair, requirements, verify};
pub use report::{ComponentState, RuntimeReport};
pub use requirement::{parse_requirements, Requirement};

/// `status` for a report that came back whole.
pub const OK_STATUS: &str = "weles_browser_runtime";

/// The component is present in the cache at its declared revision.
pub const COMPONENT_PRESENT: &str = "present";
/// The component's directory or executable is not there.
pub const COMPONENT_MISSING: &str = "missing";
/// The host could not be asked.
pub const COMPONENT_UNKNOWN: &str = "unknown";

/// Every component required by this invocation is present and a browser engine
/// is available.
pub const RUNTIME_COMPLETE: &str = "complete";
/// At least one component required by this invocation is missing.
pub const RUNTIME_INCOMPLETE: &str = "incomplete";
/// The requirement or the cache could not be read.
pub const RUNTIME_UNKNOWN: &str = "unknown";
/// The required components are present, but no browser engine can open a page.
pub const RUNTIME_BROWSER_ENGINE_MISSING: &str = "browser_engine_missing";
/// At least one browser engine could not be inspected and none is known present.
pub const RUNTIME_BROWSER_ENGINE_UNKNOWN: &str = "browser_engine_unknown";

pub const BROWSER_ENGINE_PRESENT: &str = "present";
pub const BROWSER_ENGINE_MISSING: &str = "missing";
pub const BROWSER_ENGINE_UNKNOWN: &str = "unknown";

/// Where the Weles release keeps Playwright's own requirement declaration.
pub const BROWSERS_JSON: &str = "$HOME/weles/node_modules/playwright-core/browsers.json";

/// Playwright's cache root on Darwin.
pub const CACHE_ROOT: &str = "$HOME/Library/Caches/ms-playwright";

/// The default component required when the caller names none.
///
/// This preserves the recording repair that introduced the command. Browser
/// engine readiness is reported separately and its refusal names the explicit
/// Chromium repair command.
pub const DEFAULT_COMPONENT: &str = "ffmpeg";
