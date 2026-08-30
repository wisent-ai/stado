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
//! 2. **Repair is opt-in per component, and the default is the one Weles
//!    actually takes from that cache.** Weles drives its own Chromium and
//!    Firefox, pinned by digest through `WELES_CHROMIUM_RELEASE_*` and
//!    `WELES_FIREFOX_RELEASE_*`, so Playwright's bundled browsers are not what
//!    it launches; `ffmpeg` is what it consumes from the Playwright cache, and
//!    it is the component whose absence breaks recording. Verification still
//!    reports every component the release marks `installByDefault`, so an
//!    operator sees the whole runtime rather than the one line this fault
//!    turned on, but a verify pass never downloads half a gigabyte of browsers
//!    nothing drives.

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

/// Every component the release marks `installByDefault` is present.
pub const RUNTIME_COMPLETE: &str = "complete";
/// At least one is missing.
pub const RUNTIME_INCOMPLETE: &str = "incomplete";
/// The requirement or the cache could not be read.
pub const RUNTIME_UNKNOWN: &str = "unknown";

/// Where the Weles release keeps Playwright's own requirement declaration.
pub const BROWSERS_JSON: &str = "$HOME/weles/node_modules/playwright-core/browsers.json";

/// Playwright's cache root on Darwin.
pub const CACHE_ROOT: &str = "$HOME/Library/Caches/ms-playwright";

/// The component Weles takes from the Playwright cache.
///
/// Not "all of them": the worker launches its own pinned Chromium and Firefox
/// releases, so Playwright's browsers are not what it drives. `ffmpeg` is what
/// the recording path needs and what its absence breaks.
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
    /// The components this host's worker actually consumes from the Playwright
    /// cache, and therefore the only ones the verdict turns on.
    ///
    /// Reporting every `installByDefault` component and judging every one of
    /// them are different things. Weles launches its own Chromium and Firefox,
    /// pinned by digest, so Playwright's bundled browsers are absent on a
    /// perfectly healthy worker; a verdict that failed on them would be red on
    /// every host forever, and a check that is always red is a check nobody
    /// reads. The full table stays visible so an operator can see the whole
    /// runtime; the verdict answers "can this host run a browser task".
    pub required: Vec<String>,
}

impl RuntimeReport {
    /// One word for the runtime, over the components this host consumes.
    pub fn verdict(&self) -> &'static str {
        let judged: Vec<&ComponentState> = self
            .components
            .iter()
            .filter(|component| self.required.iter().any(|name| name == &component.name))
            .collect();
        if judged.is_empty() {
            return RUNTIME_UNKNOWN;
        }
        if judged
            .iter()
            .any(|component| component.state == COMPONENT_UNKNOWN)
        {
            return RUNTIME_UNKNOWN;
        }
        if judged
            .iter()
            .any(|component| component.state == COMPONENT_MISSING)
        {
            return RUNTIME_INCOMPLETE;
        }
        RUNTIME_COMPLETE
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

    /// Why this runtime cannot run a browser task, in the words Playwright
    /// itself used, or `None`.
    ///
    /// The message names the binary and the exact path it was expected at,
    /// because that is the sentence that made the fault legible in the first
    /// place — a verdict without the path sends an operator looking.
    pub fn failure(&self, host: &str) -> Option<String> {
        match self.verdict() {
            RUNTIME_COMPLETE => None,
            RUNTIME_UNKNOWN => Some(format!(
                "{host}: the browser runtime could not be judged — the release's requirement or \
                 the Playwright cache was unreadable"
            )),
            _ => {
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
                Some(format!(
                    "{host}: the browser runtime is incomplete, so every browser task fails at \
                     `browserContext.newPage` before any navigation: {listed}. Repair it with \
                     `stado host weles-browser-runtime {host} --repair`"
                ))
            }
        }
    }

    pub fn to_report(&self, target: &str) -> Map<String, Value> {
        let mut object = Map::new();
        object.insert("host".to_string(), json!(target));
        object.insert("status".to_string(), json!(OK_STATUS));
        object.insert("runtime".to_string(), json!(self.verdict()));
        object.insert("required".to_string(), json!(self.required));
        object.insert(
            "components".to_string(),
            serde_json::to_value(&self.components).unwrap_or(Value::Null),
        );
        object
    }
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
    let script = REMOTE_REPAIR_BODY
        .replace("@COMPONENTS_B64@", &STANDARD.encode(components.join(" ")));
    let output = host_channel::run_script(target, &script, runner).await?;
    let lines: Vec<String> = output
        .stdout
        .lines()
        .filter(|line| line.starts_with("STADO_RUNTIME\t"))
        .map(|line| line.trim_start_matches("STADO_RUNTIME\t").replace('\t', ": "))
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

#[cfg(test)]
mod tests {
    use super::*;

    const DECLARATION: &str = r#"{"comment":"x","browsers":[
        {"name":"chromium","revision":"1217","installByDefault":true},
        {"name":"chromium-headless-shell","revision":"1217","installByDefault":true},
        {"name":"chromium-tip-of-tree","revision":"1417","installByDefault":false},
        {"name":"ffmpeg","revision":"1011","installByDefault":true}
    ]}"#;

    #[test]
    fn the_requirement_comes_from_the_release_with_its_exact_revisions() {
        let declared = parse_requirements(DECLARATION).unwrap();
        assert_eq!(declared.len(), 4);
        let ffmpeg = declared.iter().find(|c| c.name == "ffmpeg").unwrap();
        assert_eq!(ffmpeg.revision, "1011");
        assert!(ffmpeg.install_by_default);
        // The exact path the failure named.
        assert_eq!(
            ffmpeg.marker(),
            "$HOME/Library/Caches/ms-playwright/ffmpeg-1011/INSTALLATION_COMPLETE"
        );
    }

    #[test]
    fn a_hyphenated_component_uses_playwrights_underscore_spelling() {
        let declared = parse_requirements(DECLARATION).unwrap();
        let shell = declared
            .iter()
            .find(|c| c.name == "chromium-headless-shell")
            .unwrap();
        assert_eq!(shell.directory(), "chromium_headless_shell-1217");
    }

    #[test]
    fn a_declaration_that_is_not_playwrights_is_refused() {
        assert!(parse_requirements("{}").is_err());
        assert!(parse_requirements(r#"{"browsers":[]}"#).is_err());
        assert!(parse_requirements("not json").is_err());
    }

    fn state(name: &str, by_default: bool, state: &str) -> ComponentState {
        ComponentState {
            name: name.to_string(),
            revision: "1011".to_string(),
            install_by_default: by_default,
            expected_path: format!(
                "/Users/charles/Library/Caches/ms-playwright/{name}-1011/INSTALLATION_COMPLETE"
            ),
            state: state.to_string(),
        }
    }

    #[test]
    fn a_missing_default_component_is_incomplete_and_names_binary_and_path() {
        let report = RuntimeReport {
            components: vec![
                state("chromium", true, COMPONENT_PRESENT),
                state("ffmpeg", true, COMPONENT_MISSING),
            ],
            required: vec!["ffmpeg".to_string()],
        };
        assert_eq!(report.verdict(), RUNTIME_INCOMPLETE);
        let said = report.failure("charless-mac-mini").unwrap();
        assert!(said.contains("ffmpeg"), "{said}");
        assert!(said.contains("ms-playwright/ffmpeg-1011"), "{said}");
        assert!(said.contains("browserContext.newPage"), "{said}");
        assert!(said.contains("--repair"), "{said}");
    }

    #[test]
    fn a_component_this_host_does_not_consume_never_fails_the_verdict() {
        let report = RuntimeReport {
            components: vec![
                state("ffmpeg", true, COMPONENT_PRESENT),
                state("chromium-tip-of-tree", false, COMPONENT_MISSING),
            ],
            required: vec!["ffmpeg".to_string()],
        };
        assert_eq!(report.verdict(), RUNTIME_COMPLETE);
        assert_eq!(report.failure("h"), None);
        assert!(report.missing().is_empty());
    }

    #[test]
    fn an_unreadable_component_is_unknown_rather_than_present_or_missing() {
        let report = RuntimeReport {
            components: vec![state("ffmpeg", true, COMPONENT_UNKNOWN)],
            required: vec!["ffmpeg".to_string()],
        };
        assert_eq!(report.verdict(), RUNTIME_UNKNOWN);
        assert!(report
            .failure("h")
            .unwrap()
            .contains("could not be judged"));
    }

    #[test]
    fn the_verify_script_carries_markers_only_base64() {
        use base64::engine::general_purpose::STANDARD;
        use base64::Engine as _;
        let declared = parse_requirements(DECLARATION).unwrap();
        let payload = declared
            .iter()
            .map(|c| format!("{}|{}", c.name, c.marker()))
            .collect::<Vec<String>>()
            .join("\n");
        let script = REMOTE_VERIFY_BODY.replace("@MARKERS_B64@", &STANDARD.encode(payload.as_bytes()));
        assert!(!script.contains("ffmpeg-1011"), "{script}");
        assert!(script.contains(&STANDARD.encode(payload.as_bytes())));
    }
}
