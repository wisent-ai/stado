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
//! Nothing in Stado installed or repaired anything on a host: `host software`
//! reports what a host runs and stops there. So the alternative to this module
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

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::{host_channel, service_file_fetch, DeployError, Runner};
use crate::targets::ComputeTarget;

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

/// One component Playwright pins, as the release declares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    pub name: String,
    pub revision: String,
    pub install_by_default: bool,
}

impl Requirement {
    /// The cache directory Playwright uses for it: `<name>-<revision>` with
    /// underscores for the hyphenated multi-word names, which is the spelling
    /// Playwright itself writes (`chromium_headless_shell-1217`).
    pub fn directory(&self) -> String {
        format!("{}-{}", self.name.replace('-', "_"), self.revision)
    }

    /// The file whose existence proves the component finished installing.
    ///
    /// Playwright writes this marker after a successful install, so it
    /// distinguishes a complete component from a directory left behind by an
    /// interrupted download — which would otherwise read as present and fail
    /// at run time, exactly the kind of half-answer this fleet has been
    /// removing.
    pub fn marker(&self) -> String {
        format!("{}/{}/INSTALLATION_COMPLETE", CACHE_ROOT, self.directory())
    }
}

/// Parse Playwright's requirement declaration.
pub fn parse_requirements(body: &str) -> Result<Vec<Requirement>, DeployError> {
    let document: Value = serde_json::from_str(body)
        .map_err(|error| DeployError(format!("browsers.json did not parse: {error}")))?;
    let browsers = document
        .get("browsers")
        .and_then(Value::as_array)
        .ok_or_else(|| DeployError("browsers.json declares no browsers array".to_string()))?;
    let mut found = Vec::new();
    for entry in browsers {
        let Some(name) = entry.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some(revision) = entry.get("revision").and_then(Value::as_str) else {
            continue;
        };
        found.push(Requirement {
            name: name.to_string(),
            revision: revision.to_string(),
            install_by_default: entry
                .get("installByDefault")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
    }
    if found.is_empty() {
        return Err(DeployError(
            "browsers.json declares no component with a name and a revision".to_string(),
        ));
    }
    Ok(found)
}

/// One component's state on the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentState {
    pub name: String,
    pub revision: String,
    pub install_by_default: bool,
    /// The absolute path checked, as the host resolved it.
    pub expected_path: String,
    /// [`COMPONENT_PRESENT`], [`COMPONENT_MISSING`] or [`COMPONENT_UNKNOWN`].
    pub state: String,
}

/// The whole runtime, verified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeReport {
    pub components: Vec<ComponentState>,
    /// The components this invocation requires from the Playwright cache.
    ///
    /// This list decides `required_state`; browser-engine readiness is measured
    /// independently across Chromium, Firefox, and WebKit so satisfying a
    /// recording-only requirement can never masquerade as page readiness.
    pub required: Vec<String>,
}

impl RuntimeReport {
    /// Whether every component explicitly required by this invocation is ready.
    pub fn required_state(&self) -> &'static str {
        let mut found = false;
        let mut unknown = false;
        for component in self
            .components
            .iter()
            .filter(|component| self.required.iter().any(|name| name == &component.name))
        {
            found = true;
            if component.state == COMPONENT_MISSING {
                return RUNTIME_INCOMPLETE;
            }
            unknown |= component.state == COMPONENT_UNKNOWN;
        }
        if !found || unknown {
            RUNTIME_UNKNOWN
        } else {
            RUNTIME_COMPLETE
        }
    }

    /// Whether any Playwright Chromium, Firefox, or WebKit engine can open a page.
    pub fn browser_engine_state(&self) -> &'static str {
        let mut found = false;
        let mut unknown = false;
        for component in self
            .components
            .iter()
            .filter(|component| is_browser_engine(&component.name))
        {
            found = true;
            if component.state == COMPONENT_PRESENT {
                return BROWSER_ENGINE_PRESENT;
            }
            unknown |= component.state == COMPONENT_UNKNOWN;
        }
        if !found || unknown {
            BROWSER_ENGINE_UNKNOWN
        } else {
            BROWSER_ENGINE_MISSING
        }
    }

    /// The overall browser-task readiness shown in the report.
    pub fn verdict(&self) -> &'static str {
        match self.required_state() {
            RUNTIME_INCOMPLETE => RUNTIME_INCOMPLETE,
            RUNTIME_UNKNOWN => RUNTIME_UNKNOWN,
            _ => match self.browser_engine_state() {
                BROWSER_ENGINE_PRESENT => RUNTIME_COMPLETE,
                BROWSER_ENGINE_MISSING => RUNTIME_BROWSER_ENGINE_MISSING,
                _ => RUNTIME_BROWSER_ENGINE_UNKNOWN,
            },
        }
    }

    /// Every required component that is not there.
    pub fn missing(&self) -> Vec<&ComponentState> {
        self.components
            .iter()
            .filter(|component| {
                self.required.iter().any(|name| name == &component.name)
                    && component.state == COMPONENT_MISSING
            })
            .collect()
    }

    /// Why this host cannot open a page, or `None`.
    pub fn failure(&self, host: &str) -> Option<String> {
        match self.required_state() {
            RUNTIME_UNKNOWN => Some(format!(
                "{host}: the required Playwright components could not be judged because the \
                 release requirement or cache was unreadable"
            )),
            RUNTIME_INCOMPLETE => {
                let missing = self.missing();
                let listed = missing
                    .iter()
                    .map(|component| {
                        format!(
                            "{} {} expected at {}",
                            component.name, component.revision, component.expected_path
                        )
                    })
                    .collect::<Vec<String>>()
                    .join("; ");
                let component_flags = missing
                    .iter()
                    .map(|component| format!(" --component {}", component.name))
                    .collect::<String>();
                Some(format!(
                    "{host}: the browser runtime is incomplete, so every browser task fails at \
                     `browserContext.newPage` before any navigation: {listed}; repair it with \
                     `stado host weles-browser-runtime {host}{component_flags} --repair`."
                ))
            }
            _ => match self.browser_engine_state() {
                BROWSER_ENGINE_MISSING => Some(format!(
                    "{host}: required Playwright components are complete, but no Chromium, \
                     Firefox, or WebKit engine is installed, so `browserContext.newPage` cannot \
                     open a page; install Chromium with `stado host weles-browser-runtime {host} \
                     --component chromium --repair`."
                )),
                BROWSER_ENGINE_UNKNOWN => Some(format!(
                    "{host}: required Playwright components are complete, but browser-engine \
                     readiness could not be judged, so `browserContext.newPage` is not known to \
                     work."
                )),
                _ => None,
            },
        }
    }

    pub fn to_report(&self, target: &str) -> Map<String, Value> {
        let mut object = Map::new();
        object.insert("host".to_string(), json!(target));
        object.insert("status".to_string(), json!(OK_STATUS));
        object.insert("runtime".to_string(), json!(self.verdict()));
        object.insert("required".to_string(), json!(self.required));
        object.insert("required_state".to_string(), json!(self.required_state()));
        object.insert(
            "browser_engine_state".to_string(),
            json!(self.browser_engine_state()),
        );
        object.insert(
            "components".to_string(),
            serde_json::to_value(&self.components).unwrap_or(Value::Null),
        );
        object
    }
}

fn is_browser_engine(name: &str) -> bool {
    name == "webkit" || name.starts_with("chromium") || name.starts_with("firefox")
}

/// Read the release's requirement, byte-exact.
pub async fn requirements(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<Vec<Requirement>, DeployError> {
    let fetched = service_file_fetch::fetch_file(target, BROWSERS_JSON, runner).await?;
    if !fetched.ok() {
        return Err(DeployError(format!(
            "{}: could not read {BROWSERS_JSON}, so what browser runtime this release needs is \
             unknown: {} ({})",
            target.name, fetched.report.file_state, fetched.integrity
        )));
    }
    parse_requirements(&String::from_utf8_lossy(&fetched.content))
}

/// The remote program that reports which cache directories are complete.
///
/// One `test -f` per marker, and nothing else: this reads presence and never
/// downloads, so a verify pass on a healthy host costs one round trip and
/// changes nothing.
const REMOTE_VERIFY_BODY: &str = r##"set -eu
LC_ALL=C
export LC_ALL
decode=-D
if [ "$(uname)" = "Linux" ]; then decode=--decode; fi
markers=$(printf '%s' '@MARKERS_B64@' | /usr/bin/base64 "$decode")
printf '{"components":['
first=1
printf '%s\n' "$markers" | while IFS= read -r entry; do
  [ -n "$entry" ] || continue
  name=${entry%%|*}
  path=${entry#*|}
  case "$path" in
    '$HOME'/*) resolved="$HOME/${path#\$HOME/}" ;;
    *) resolved="$path" ;;
  esac
  if [ -f "$resolved" ]; then state=present; else state=missing; fi
  if [ "$first" = 1 ]; then first=0; else printf ','; fi
  printf '{"name":"%s","path":"%s","state":"%s"}' "$name" "$resolved" "$state"
done
printf ']}\n'
"##;

/// The remote program that completes the runtime.
///
/// `npx playwright install <component>` run from the installed release, so the
/// Playwright that resolves the download is the one the release pins rather
/// than whatever a global npm happens to hold. The node that runs it is the
/// worker's own.
const REMOTE_REPAIR_BODY: &str = r##"set -eu
LC_ALL=C
export LC_ALL
decode=-D
if [ "$(uname)" = "Linux" ]; then decode=--decode; fi
components=$(printf '%s' '@COMPONENTS_B64@' | /usr/bin/base64 "$decode")
release="$HOME/weles"
if [ ! -d "$release" ]; then
  printf 'STADO_RUNTIME\tfailed\tno release checkout at %s\n' "$release"
  exit 0
fi
node_bin=""
for candidate in /opt/homebrew/bin/node /usr/local/bin/node /usr/bin/node; do
  if [ -x "$candidate" ]; then node_bin="$candidate"; break; fi
done
if [ -z "$node_bin" ]; then
  printf 'STADO_RUNTIME\tfailed\tno node on this host to run the installer\n'
  exit 0
fi
cli="$release/node_modules/playwright-core/cli.js"
if [ ! -f "$cli" ]; then
  printf 'STADO_RUNTIME\tfailed\tthe release carries no playwright-core cli at %s\n' "$cli"
  exit 0
fi
cd "$release"
for component in $components; do
  if out=$("$node_bin" "$cli" install "$component" 2>&1); then
    printf 'STADO_RUNTIME\tinstalled\t%s\n' "$component"
  else
    printf 'STADO_RUNTIME\tfailed\t%s: %s\n' "$component" "$(printf '%s' "$out" | /usr/bin/tail -n 3 | /usr/bin/tr '\n\t' '  ')"
  fi
done
"##;

/// Verify every declared component against the host's cache.
pub async fn verify(
    target: &ComputeTarget,
    declared: &[Requirement],
    required: &[String],
    runner: &Runner,
) -> Result<RuntimeReport, DeployError> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;
    let payload = declared
        .iter()
        .map(|component| format!("{}|{}", component.name, component.marker()))
        .collect::<Vec<String>>()
        .join("\n");
    let script = REMOTE_VERIFY_BODY.replace("@MARKERS_B64@", &STANDARD.encode(payload.as_bytes()));
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: {}",
            target.name,
            host_channel::last_error_line(&output, "ssh failed")
        )));
    }
    let line = output
        .stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with('{'))
        .ok_or_else(|| DeployError("runtime script produced no JSON report".to_string()))?;
    let parsed: Value = serde_json::from_str(line)
        .map_err(|error| DeployError(format!("runtime script returned bad JSON: {error}")))?;
    let rows = parsed
        .get("components")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let components = declared
        .iter()
        .map(|component| {
            let row = rows.iter().find(|row| {
                row.get("name").and_then(Value::as_str) == Some(component.name.as_str())
            });
            ComponentState {
                name: component.name.clone(),
                revision: component.revision.clone(),
                install_by_default: component.install_by_default,
                expected_path: row
                    .and_then(|row| row.get("path"))
                    .and_then(Value::as_str)
                    .unwrap_or(&component.marker())
                    .to_string(),
                state: row
                    .and_then(|row| row.get("state"))
                    .and_then(Value::as_str)
                    .unwrap_or(COMPONENT_UNKNOWN)
                    .to_string(),
            }
        })
        .collect();
    Ok(RuntimeReport {
        components,
        required: required.to_vec(),
    })
}

/// Install the named components on the host.
pub async fn repair(
    target: &ComputeTarget,
    components: &[String],
    runner: &Runner,
) -> Result<Vec<String>, DeployError> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;
    for component in components {
        if !component
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
        {
            return Err(DeployError(format!(
                "{component:?} is not a Playwright component name"
            )));
        }
    }
    let script =
        REMOTE_REPAIR_BODY.replace("@COMPONENTS_B64@", &STANDARD.encode(components.join(" ")));
    let output = host_channel::run_script(target, &script, runner).await?;
    let lines: Vec<String> = output
        .stdout
        .lines()
        .filter(|line| line.starts_with("STADO_RUNTIME\t"))
        .map(|line| {
            line.trim_start_matches("STADO_RUNTIME\t")
                .replace('\t', ": ")
        })
        .collect();
    if lines.is_empty() {
        return Err(DeployError(format!(
            "{}: the installer reported nothing: {}",
            target.name,
            host_channel::last_error_line(&output, "no output")
        )));
    }
    Ok(lines)
}
