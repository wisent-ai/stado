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
//!    [`ComputeTarget::mobile_runtime`], so a host that is not a mobile
//!    placement declares nothing and is not judged, and the version this
//!    verifies is the version the fleet asked for rather than a constant in
//!    whatever checkout an operator happens to run.
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

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::{host_channel, DeployError, Runner};
use crate::targets::{ComputeTarget, MobileRuntime};

/// `status` for a report that came back whole.
pub const OK_STATUS: &str = "mobile_runtime";

/// The component is on the host at one of its declared paths.
pub const COMPONENT_PRESENT: &str = "present";
/// The component is not at any path this fleet installs it at.
pub const COMPONENT_MISSING: &str = "missing";
/// The component is present but reports a different version than declared.
pub const COMPONENT_DRIFTED: &str = "drifted";
/// The host could not be asked.
pub const COMPONENT_UNKNOWN: &str = "unknown";

/// Every declared component is present at its declared version.
pub const RUNTIME_COMPLETE: &str = "complete";
/// At least one is missing or drifted.
pub const RUNTIME_INCOMPLETE: &str = "incomplete";
/// The requirement or the host could not be read.
pub const RUNTIME_UNKNOWN: &str = "unknown";

/// The npm prefix repair installs the Appium server under.
///
/// `~/.npm-global` is the first candidate [`super::host_exec`]'s table names
/// for `appium`, so the program this installs is the program that allowlist
/// finds. A global install without an explicit prefix writes wherever the
/// host's npm happens to be configured, which on a Homebrew node is a
/// directory the fleet's probe order does not carry.
pub const NPM_PREFIX: &str = "$HOME/.npm-global";

/// Where repair unpacks Android platform-tools.
///
/// The first candidate the allowlist names for `adb`, and the location the
/// vendor's own SDK layout uses, so a later `sdkmanager` on this host manages
/// the same tree rather than a second copy.
pub const ANDROID_SDK_ROOT: &str = "$HOME/Library/Android/sdk";

/// One component of the runtime, as the host reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentState {
    /// `appium`, `adb`, or `driver:<name>`.
    pub name: String,
    /// What the declaration asks for: a version, or `required`.
    pub declared: String,
    /// Absolute path the host resolved it at, or the candidate list it tried.
    pub path: String,
    /// The version the program itself reported, when it ran.
    pub observed: String,
    /// [`COMPONENT_PRESENT`], `_MISSING`, `_DRIFTED` or `_UNKNOWN`.
    pub state: String,
}

/// Every component of one host's declared mobile runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeReport {
    pub components: Vec<ComponentState>,
}

impl RuntimeReport {
    /// `complete` only when every component is present at its declaration.
    pub fn verdict(&self) -> &'static str {
        if self.components.is_empty() {
            return RUNTIME_UNKNOWN;
        }
        if self
            .components
            .iter()
            .any(|component| component.state == COMPONENT_UNKNOWN)
        {
            return RUNTIME_UNKNOWN;
        }
        if self
            .components
            .iter()
            .all(|component| component.state == COMPONENT_PRESENT)
        {
            return RUNTIME_COMPLETE;
        }
        RUNTIME_INCOMPLETE
    }

    /// Components a repair would have to act on.
    pub fn incomplete(&self) -> Vec<&ComponentState> {
        self.components
            .iter()
            .filter(|component| component.state != COMPONENT_PRESENT)
            .collect()
    }

    /// One sentence naming the host and the exact disagreement, or `None`
    /// when the runtime is complete. Silence is never rounded to agreement:
    /// an unknown component fails here exactly as a missing one does.
    pub fn failure(&self, host: &str) -> Option<String> {
        let broken = self.incomplete();
        if broken.is_empty() {
            return None;
        }
        Some(format!(
            "{host}: mobile runtime {} — {}",
            self.verdict(),
            broken
                .iter()
                .map(|component| format!(
                    "{} is {} (declared {}, looked at {})",
                    component.name, component.state, component.declared, component.path
                ))
                .collect::<Vec<String>>()
                .join("; ")
        ))
    }

    /// The `--json` document.
    pub fn to_report(&self, target: &str) -> Map<String, Value> {
        let mut object = Map::new();
        object.insert("status".to_string(), json!(OK_STATUS));
        object.insert("target".to_string(), json!(target));
        object.insert("runtime".to_string(), json!(self.verdict()));
        object.insert("components".to_string(), json!(self.components));
        object
    }
}

/// The requirement this host declares, or `None` when it declares none.
///
/// Absence is not a failure and must not be rounded into one: a host that is
/// not a mobile placement is the default, and judging every host against a
/// runtime only two of them need is how an operator learns to ignore the
/// report.
pub fn requirement(target: &ComputeTarget) -> Option<&MobileRuntime> {
    target.mobile_runtime.as_ref()
}

/// Read every component off the host in one round trip.
///
/// One script rather than a probe per component: each round trip is an ssh
/// connection, and a five-connection read of one host's runtime is four
/// connections spent on formatting.
const REMOTE_VERIFY_BODY: &str = r##"set -eu
LC_ALL=C
export LC_ALL
resolve() {
  for candidate in "$@"; do
    if [ -x "$candidate" ]; then printf '%s' "$candidate"; return 0; fi
  done
  printf ''
}
json_escape() {
  printf '%s' "$1" | /usr/bin/sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' | /usr/bin/tr -d '\n\r\t'
}
# The same candidate order deploy::host_exec probes, so the two readers cannot
# name different binaries on this machine.
appium=$(resolve "$HOME/.npm-global/bin/appium" "$HOME/.local/bin/appium" /opt/homebrew/bin/appium /usr/local/bin/appium)
adb=$(resolve "$HOME/Library/Android/sdk/platform-tools/adb" /opt/homebrew/bin/adb /usr/local/bin/adb)
appium_version=''
drivers=''
if [ -n "$appium" ]; then
  appium_version=$("$appium" --version 2>/dev/null | /usr/bin/tr -d '\n\r' || printf '')
  # `driver list --installed` writes its table on stderr in some Appium
  # builds, so both streams are read and the names are matched out of it.
  drivers=$("$appium" driver list --installed 2>&1 | /usr/bin/tr -d '\r' | /usr/bin/tr '\n' ' ' || printf '')
fi
adb_version=''
if [ -n "$adb" ]; then
  adb_version=$("$adb" version 2>/dev/null | /usr/bin/head -n 1 | /usr/bin/tr -d '\n\r' || printf '')
fi
printf '{"appium_path":"%s","appium_version":"%s","drivers":"%s","adb_path":"%s","adb_version":"%s"}\n' \
  "$(json_escape "$appium")" \
  "$(json_escape "$appium_version")" \
  "$(json_escape "$drivers")" \
  "$(json_escape "$adb")" \
  "$(json_escape "$adb_version")"
"##;

/// Install exactly what the declaration asks for, into the login user's home.
///
/// Every step is idempotent: `npm install -g` at a pinned version is a no-op
/// on a host already at it, `appium driver install` declines a driver already
/// present, and platform-tools is re-unpacked over its own tree. So a
/// provisioned host pays a no-op and a fresh one provisions itself, which is
/// the property [`super::release_submit`]'s toolchain provisioning argues for.
const REMOTE_REPAIR_BODY: &str = r##"set -eu
LC_ALL=C
export LC_ALL
decode=-D
if [ "$(uname)" = "Linux" ]; then decode=--decode; fi
appium_version=$(printf '%s' '@APPIUM_B64@' | /usr/bin/base64 "$decode")
drivers=$(printf '%s' '@DRIVERS_B64@' | /usr/bin/base64 "$decode")
platform_tools=$(printf '%s' '@PLATFORM_TOOLS_B64@' | /usr/bin/base64 "$decode")
prefix="$HOME/.npm-global"
sdk="$HOME/Library/Android/sdk"
npm_bin=''
for candidate in /opt/homebrew/bin/npm /usr/local/bin/npm /usr/bin/npm; do
  if [ -x "$candidate" ]; then npm_bin="$candidate"; break; fi
done
if [ -n "$appium_version" ]; then
  if [ -z "$npm_bin" ]; then
    printf 'STADO_RUNTIME\tfailed\tappium: no npm on this host to install it with\n'
  else
    node_dir=$(/usr/bin/dirname "$npm_bin")
    PATH="$node_dir:$PATH"
    export PATH
    /bin/mkdir -p "$prefix"
    if out=$("$npm_bin" install --global --prefix "$prefix" "appium@$appium_version" 2>&1); then
      printf 'STADO_RUNTIME\tinstalled\tappium@%s\n' "$appium_version"
    else
      printf 'STADO_RUNTIME\tfailed\tappium@%s: %s\n' "$appium_version" "$(printf '%s' "$out" | /usr/bin/tail -n 3 | /usr/bin/tr '\n\t' '  ')"
    fi
  fi
fi
appium_bin=''
for candidate in "$prefix/bin/appium" "$HOME/.local/bin/appium" /opt/homebrew/bin/appium /usr/local/bin/appium; do
  if [ -x "$candidate" ]; then appium_bin="$candidate"; break; fi
done
for driver in $drivers; do
  if [ -z "$appium_bin" ]; then
    printf 'STADO_RUNTIME\tfailed\tdriver %s: no appium on this host to install it into\n' "$driver"
    continue
  fi
  node_dir=$(/usr/bin/dirname "$npm_bin")
  PATH="$node_dir:$PATH"
  export PATH
  if "$appium_bin" driver list --installed 2>&1 | /usr/bin/grep -q -- "$driver"; then
    printf 'STADO_RUNTIME\tpresent\tdriver %s\n' "$driver"
    continue
  fi
  if out=$("$appium_bin" driver install "$driver" 2>&1); then
    printf 'STADO_RUNTIME\tinstalled\tdriver %s\n' "$driver"
  else
    printf 'STADO_RUNTIME\tfailed\tdriver %s: %s\n' "$driver" "$(printf '%s' "$out" | /usr/bin/tail -n 3 | /usr/bin/tr '\n\t' '  ')"
  fi
done
if [ "$platform_tools" = "yes" ]; then
  if [ -x "$sdk/platform-tools/adb" ]; then
    printf 'STADO_RUNTIME\tpresent\tplatform-tools\n'
  else
    /bin/mkdir -p "$sdk"
    archive="$sdk/platform-tools-latest-darwin.zip"
    if [ "$(uname)" = "Linux" ]; then
      archive="$sdk/platform-tools-latest-linux.zip"
      url='https://dl.google.com/android/repository/platform-tools-latest-linux.zip'
    else
      url='https://dl.google.com/android/repository/platform-tools-latest-darwin.zip'
    fi
    /bin/rm -f "$archive"
    if /usr/bin/curl -fsS -o "$archive" "$url"; then
      if out=$(cd "$sdk" && /usr/bin/unzip -o -q "$archive" 2>&1); then
        printf 'STADO_RUNTIME\tinstalled\tplatform-tools\n'
      else
        printf 'STADO_RUNTIME\tfailed\tplatform-tools unpack: %s\n' "$(printf '%s' "$out" | /usr/bin/tail -n 2 | /usr/bin/tr '\n\t' '  ')"
      fi
      /bin/rm -f "$archive"
    else
      printf 'STADO_RUNTIME\tfailed\tplatform-tools: could not fetch %s\n' "$url"
    fi
  fi
fi
"##;

/// Everything the host said about its runtime, judged against the
/// declaration.
pub async fn verify(
    target: &ComputeTarget,
    declared: &MobileRuntime,
    runner: &Runner,
) -> Result<RuntimeReport, DeployError> {
    let output = host_channel::run_script(target, REMOTE_VERIFY_BODY, runner).await?;
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
    let field = |name: &str| {
        parsed
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };

    let mut components = Vec::new();

    let appium_path = field("appium_path");
    let appium_version = field("appium_version");
    // The declaration is an exact version, so a present-but-different Appium
    // is `drifted` and not `present`: a placement pinned to a driver protocol
    // that its server does not speak fails at the first command, which is the
    // failure this is meant to precede rather than reproduce.
    let appium_state = if appium_path.is_empty() {
        COMPONENT_MISSING
    } else if appium_version.is_empty() {
        COMPONENT_UNKNOWN
    } else if appium_version.trim() == declared.appium.trim() {
        COMPONENT_PRESENT
    } else {
        COMPONENT_DRIFTED
    };
    components.push(ComponentState {
        name: "appium".to_string(),
        declared: declared.appium.clone(),
        path: if appium_path.is_empty() {
            format!("{NPM_PREFIX}/bin/appium and 3 more")
        } else {
            appium_path
        },
        observed: appium_version,
        state: appium_state.to_string(),
    });

    let installed_drivers = field("drivers");
    for driver in &declared.drivers {
        // Matched as a word out of the driver table rather than parsed: the
        // table's columns differ between Appium 2 and 3, and the one fact
        // needed is whether the name is in it.
        let present = installed_drivers
            .split(|character: char| !character.is_ascii_alphanumeric() && character != '-')
            .any(|word| word == driver.as_str());
        components.push(ComponentState {
            name: format!("driver:{driver}"),
            declared: "required".to_string(),
            path: "appium driver list --installed".to_string(),
            observed: if present {
                "installed".to_string()
            } else {
                String::new()
            },
            state: if present {
                COMPONENT_PRESENT.to_string()
            } else {
                COMPONENT_MISSING.to_string()
            },
        });
    }

    if declared.platform_tools {
        let adb_path = field("adb_path");
        let adb_version = field("adb_version");
        let adb_state = if adb_path.is_empty() {
            COMPONENT_MISSING
        } else if adb_version.is_empty() {
            COMPONENT_UNKNOWN
        } else {
            COMPONENT_PRESENT
        };
        components.push(ComponentState {
            name: "adb".to_string(),
            declared: "required".to_string(),
            path: if adb_path.is_empty() {
                format!("{ANDROID_SDK_ROOT}/platform-tools/adb and 2 more")
            } else {
                adb_path
            },
            observed: adb_version,
            state: adb_state.to_string(),
        });
    }

    Ok(RuntimeReport { components })
}

/// Install the declared runtime on the host, and report every step.
pub async fn repair(
    target: &ComputeTarget,
    declared: &MobileRuntime,
    runner: &Runner,
) -> Result<Vec<String>, DeployError> {
    use base64::engine::general_purpose::STANDARD;
    use base64::Engine as _;
    // The declaration reaches a shell, so it is checked before it does. Not
    // quoting-as-defence: a version or driver name is a coordinate, and one
    // carrying a shell character is a malformed declaration to refuse, for
    // the reason `host_exec` refuses an argument a shell would interpret.
    if declared.appium.trim().is_empty()
        || !declared
            .appium
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".-+".contains(character))
    {
        return Err(DeployError(format!(
            "{:?} is not an Appium version coordinate",
            declared.appium
        )));
    }
    for driver in &declared.drivers {
        if driver.trim().is_empty()
            || !driver
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
        {
            return Err(DeployError(format!(
                "{driver:?} is not an Appium driver name"
            )));
        }
    }
    let script = REMOTE_REPAIR_BODY
        .replace("@APPIUM_B64@", &STANDARD.encode(declared.appium.as_bytes()))
        .replace(
            "@DRIVERS_B64@",
            &STANDARD.encode(declared.drivers.join(" ").as_bytes()),
        )
        .replace(
            "@PLATFORM_TOOLS_B64@",
            &STANDARD.encode(if declared.platform_tools { "yes" } else { "no" }),
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    fn declared() -> MobileRuntime {
        MobileRuntime {
            appium: "3.2.1".to_string(),
            drivers: vec!["xcuitest".to_string(), "uiautomator2".to_string()],
            platform_tools: true,
            extra: Map::new(),
        }
    }

    fn component(name: &str, state: &str) -> ComponentState {
        ComponentState {
            name: name.to_string(),
            declared: "required".to_string(),
            path: "/x".to_string(),
            observed: String::new(),
            state: state.to_string(),
        }
    }

    #[test]
    fn a_missing_component_makes_the_runtime_incomplete_and_names_itself() {
        let report = RuntimeReport {
            components: vec![
                component("appium", COMPONENT_PRESENT),
                component("adb", COMPONENT_MISSING),
            ],
        };
        assert_eq!(report.verdict(), RUNTIME_INCOMPLETE);
        let failure = report.failure("lukasz-macbook").expect("a failure");
        assert!(failure.contains("lukasz-macbook"));
        assert!(failure.contains("adb is missing"));
        // The component that was fine is not named as a fault.
        assert!(!failure.contains("appium is"));
    }

    #[test]
    fn silence_from_one_component_is_never_rounded_to_agreement() {
        let report = RuntimeReport {
            components: vec![
                component("appium", COMPONENT_PRESENT),
                component("adb", COMPONENT_UNKNOWN),
            ],
        };
        assert_eq!(report.verdict(), RUNTIME_UNKNOWN);
        assert!(report.failure("h").is_some());
    }

    #[test]
    fn a_complete_runtime_has_no_failure() {
        let report = RuntimeReport {
            components: vec![
                component("appium", COMPONENT_PRESENT),
                component("driver:xcuitest", COMPONENT_PRESENT),
            ],
        };
        assert_eq!(report.verdict(), RUNTIME_COMPLETE);
        assert_eq!(report.failure("h"), None);
    }

    #[test]
    fn an_empty_report_is_unknown_rather_than_complete() {
        let report = RuntimeReport { components: vec![] };
        assert_eq!(report.verdict(), RUNTIME_UNKNOWN);
    }

    #[test]
    fn a_version_that_is_not_a_coordinate_is_refused_before_it_reaches_a_shell() {
        let runner = crate::deploy::production_runner();
        let mut requirement = declared();
        requirement.appium = "3.2.1; rm -rf /".to_string();
        let target = ComputeTarget {
            name: "h".to_string(),
            kind: "local".to_string(),
            ..serde_json::from_value(serde_json::json!({"name":"h","kind":"local"}))
                .expect("a minimal target")
        };
        let error = futures::executor::block_on(repair(&target, &requirement, &runner))
            .expect_err("a refusal");
        assert!(error.0.contains("not an Appium version coordinate"));
    }

    #[test]
    fn a_driver_name_that_is_not_a_name_is_refused_too() {
        let runner = crate::deploy::production_runner();
        let mut requirement = declared();
        requirement.drivers = vec!["xcuitest && curl evil".to_string()];
        let target: ComputeTarget =
            serde_json::from_value(serde_json::json!({"name":"h","kind":"local"}))
                .expect("a minimal target");
        let error = futures::executor::block_on(repair(&target, &requirement, &runner))
            .expect_err("a refusal");
        assert!(error.0.contains("not an Appium driver name"));
    }

    #[test]
    fn the_declaration_round_trips_through_the_registry_shape() {
        let parsed: MobileRuntime = serde_json::from_value(serde_json::json!({
            "appium": "3.2.1",
            "drivers": ["xcuitest", "uiautomator2"],
            "platform_tools": true,
            "future_key": 7
        }))
        .expect("a declaration");
        assert_eq!(parsed.appium, "3.2.1");
        assert_eq!(parsed.drivers.len(), 2);
        assert!(parsed.platform_tools);
        // A newer publisher's key survives a rewrite from this checkout.
        assert!(parsed.extra.contains_key("future_key"));
    }

    #[test]
    fn a_host_that_declares_nothing_is_not_judged() {
        let target: ComputeTarget =
            serde_json::from_value(serde_json::json!({"name":"h","kind":"local"}))
                .expect("a minimal target");
        assert!(requirement(&target).is_none());
    }
}
