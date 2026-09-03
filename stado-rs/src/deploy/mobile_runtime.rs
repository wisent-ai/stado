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

/// A driver the host carries, that this declaration says nothing about, and
/// that the installed server itself calls incompatible with it.
///
/// Reported, counted and visible, and it does NOT decide the verdict. That is
/// the split [`crate::host_software`] already argues for: failing a host over
/// a program nothing declares is how an operator learns to write `|| true`
/// after the command, at which point the drift the check exists to catch
/// stops being noticed. But leaving it out of the report entirely is how
/// `charless-mac-mini` kept a `mac2@1.20.5` that the server calls
/// incompatible, ready to deadlock npm for whichever install came next.
pub const COMPONENT_UNDECLARED_INCOMPATIBLE: &str = "incompatible-undeclared";

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

/// Wall clock a repair is allowed, and why it is not the shared default.
///
/// [`host_channel::run_script`]'s bound is 120 seconds, which is right for
/// the reads every other host command makes and wrong for this one: a single
/// `appium driver install uiautomator2` fetches the driver, its dependency
/// tree and its bundled server APKs, and the first attempt at this repair
/// died at exactly that bound with the driver half-installed. A timeout
/// shorter than the operation does not protect anything — it converts a slow
/// success into an indeterminate state — so the bound is sized to the work
/// and stays a bound, because an install that has not finished in a quarter
/// of an hour is a fault to report and not a download to keep waiting on.
const REPAIR_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(900);

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
    /// Only components this host DECLARED decide anything.
    ///
    /// An undeclared driver the server calls incompatible is in the report
    /// and not in the gate, for the reason
    /// [`COMPONENT_UNDECLARED_INCOMPATIBLE`] gives.
    fn judged(&self) -> impl Iterator<Item = &ComponentState> {
        self.components
            .iter()
            .filter(|component| component.state != COMPONENT_UNDECLARED_INCOMPATIBLE)
    }

    /// `complete` only when every declared component is present at its
    /// declaration.
    pub fn verdict(&self) -> &'static str {
        if self.judged().next().is_none() {
            return RUNTIME_UNKNOWN;
        }
        if self
            .judged()
            .any(|component| component.state == COMPONENT_UNKNOWN)
        {
            return RUNTIME_UNKNOWN;
        }
        if self
            .judged()
            .all(|component| component.state == COMPONENT_PRESENT)
        {
            return RUNTIME_COMPLETE;
        }
        RUNTIME_INCOMPLETE
    }

    /// Components a repair would have to act on.
    pub fn incomplete(&self) -> Vec<&ComponentState> {
        self.judged()
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
appium=$(resolve @APPIUM_CANDIDATES@)
adb=$(resolve @ADB_CANDIDATES@)
# `appium` is a JavaScript shim whose first line is `#!/usr/bin/env node`, so
# on a channel whose PATH does not carry Node it answers `env: node: No such
# file or directory` -- present, runnable, and reported as broken. That is
# exactly what charless-mac-mini answered on the first repair: the binary was
# installed at ~/.npm-global/bin/appium and `--version` came back empty, so
# the runtime read `unknown`. The interpreter a shim needs is a sibling of the
# Node the fleet installs, which is the argument `host_exec::candidate_script`
# already makes for the same programs.
node_dir=''
for candidate in @NODE_CANDIDATES@; do
  if [ -x "$candidate" ]; then node_dir=$(/usr/bin/dirname "$candidate"); break; fi
done
if [ -n "$node_dir" ]; then
  PATH="$node_dir:$PATH"
  export PATH
fi
appium_version=''
drivers=''
warnings=''
if [ -n "$appium" ]; then
  appium_version=$("$appium" --version 2>/dev/null | /usr/bin/tr -d '\n\r' || printf '')
  # `driver list --installed` writes its table on stderr in some Appium
  # builds, so both streams are read and the names are matched out of it.
  listing=$("$appium" driver list --installed 2>&1 | /usr/bin/tr -d '\r' || printf '')
  drivers=$(printf '%s' "$listing" | /usr/bin/tr '\n' ' ')
  # The server's incompatibility verdicts, kept APART from the listing.
  # Joined into one blob they cannot be told apart, and the reader ends up
  # quoting a driver's whole table back at the operator as if it were the
  # sentence about one driver. One field per question.
  warnings=$(printf '%s' "$listing" | /usr/bin/grep -E 'may be incompatible' | /usr/bin/tr '\n' ' ' || printf '')
fi
adb_version=''
if [ -n "$adb" ]; then
  adb_version=$("$adb" version 2>/dev/null | /usr/bin/head -n 1 | /usr/bin/tr -d '\n\r' || printf '')
fi
printf '{"appium_path":"%s","appium_version":"%s","drivers":"%s","warnings":"%s","adb_path":"%s","adb_version":"%s"}\n' \
  "$(json_escape "$appium")" \
  "$(json_escape "$appium_version")" \
  "$(json_escape "$drivers")" \
  "$(json_escape "$warnings")" \
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
# The reason, not the epilogue. npm ends a failed install with three lines
# naming its own log files, so a blind `tail` reports where the answer was
# written on a host nobody may read files on, and hides the answer. Prefer the
# lines that carry the diagnosis, and fall back to the tail only when none do.
diagnose() {
  said=$(printf '%s' "$1" | /usr/bin/grep -E 'ERESOLVE|npm error' 2>/dev/null \
    | /usr/bin/grep -v -E '_logs|A complete log|For a full report' \
    | /usr/bin/head -n 8 | /usr/bin/tr '\n\t' '  ')
  if [ -z "$said" ]; then
    said=$(printf '%s' "$1" | /usr/bin/tail -n 3 | /usr/bin/tr '\n\t' '  ')
  fi
  printf '%s' "$said"
}
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
      printf 'STADO_RUNTIME\tfailed\tappium@%s: %s\n' "$appium_version" "$(diagnose "$out")"
    fi
  fi
fi
appium_bin=''
for candidate in "$prefix/bin/appium" @APPIUM_CANDIDATES@; do
  if [ -x "$candidate" ]; then appium_bin="$candidate"; break; fi
done
# The declared server and the installed driver tree have to agree, and on this
# fleet they did not. `$APPIUM_HOME` outlives any one server: this host still
# carried `appium-mac2-driver@1.20.5`, whose peer is `appium@^2.4.1`, from an
# Appium 2 era, and npm resolves the whole extension tree at once -- so an
# undeclared driver nobody asked about refused every install into it with
# ERESOLVE. Updating the installed set is the conservative repair: it keeps
# every driver the host has, including ones this declaration says nothing
# about, and only moves them onto versions the declared server can host.
# Removing the blocker instead would be Stado deciding that a capability it
# was not asked about is expendable.
# The deadlock needs one, and only one, relaxed resolution. The blocker is a
# STALE pin: `appium-mac2-driver@1.20.5` demands `appium@^2.4.1`, and npm
# resolves the whole extension tree at once, so while it is in the tree even
# `driver update` cannot run -- the command that would remove the conflict is
# refused by the conflict. `NPM_CONFIG_LEGACY_PEER_DEPS` is set for the
# update alone, which lets the update land the CURRENT versions of every
# installed driver, all of which declare `appium@^3.0.0-rc.2`. The tree is
# then consistent on its own terms and the install that follows runs under
# ordinary resolution. Nothing is uninstalled: an undeclared driver is
# updated and kept, never removed, because deciding a capability nobody
# asked about is expendable is the operator's call and not this command's.
update_installed_drivers() {
  if [ "$drivers_updated" = "yes" ]; then return 0; fi
  drivers_updated=yes
  out=$(NPM_CONFIG_LEGACY_PEER_DEPS=true "$appium_bin" driver update installed --unsafe 2>&1) \
    || printf 'STADO_RUNTIME\tfailed\tdriver update: %s\n' "$(diagnose "$out")"
  # The claim is checked against the world before it is made. The first
  # version of this printed "updated" on a zero exit, and on
  # charless-mac-mini that zero exit sat beside a `mac2` still at 1.20.5 with
  # the server still calling it incompatible -- a report of a state nobody
  # had verified, which is the whole defect class this module exists to
  # avoid. So: re-read the server's own verdict and say what it says.
  after=$("$appium_bin" driver list --installed 2>&1 || printf '')
  case "$after" in
    *"potential problem"*)
      printf 'STADO_RUNTIME\tunresolved\tdriver update ran and the server still reports: %s\n' \
        "$(printf '%s' "$after" | /usr/bin/grep -E 'may be incompatible' | /usr/bin/head -n 3 | /usr/bin/tr '\n\t' '  ')"
      ;;
    *)
      printf 'STADO_RUNTIME\tupdated\tinstalled driver set; the server now reports no incompatible driver\n'
      ;;
  esac
}
drivers_updated=no
for driver in $drivers; do
  if [ -z "$appium_bin" ]; then
    printf 'STADO_RUNTIME\tfailed\tdriver %s: no appium on this host to install it into\n' "$driver"
    continue
  fi
  node_dir=$(/usr/bin/dirname "$npm_bin")
  PATH="$node_dir:$PATH"
  export PATH
  if "$appium_bin" driver list --installed 2>&1 | /usr/bin/grep -q -- "$driver"; then
    # Present, but presence is not agreement: a driver installed against an
    # older server is reported here and judged by the verify pass that
    # follows, which reads each driver's own version.
    printf 'STADO_RUNTIME\tpresent\tdriver %s\n' "$driver"
    continue
  fi
  if out=$("$appium_bin" driver install "$driver" 2>&1); then
    printf 'STADO_RUNTIME\tinstalled\tdriver %s\n' "$driver"
    continue
  fi
  case "$out" in
    *ERESOLVE*)
      update_installed_drivers
      if retry=$("$appium_bin" driver install "$driver" 2>&1); then
        printf 'STADO_RUNTIME\tinstalled\tdriver %s\n' "$driver"
      else
        printf 'STADO_RUNTIME\tfailed\tdriver %s: %s\n' "$driver" "$(diagnose "$retry")"
      fi
      ;;
    *)
      printf 'STADO_RUNTIME\tfailed\tdriver %s: %s\n' "$driver" "$(diagnose "$out")"
      ;;
  esac
done
# The server's own verdict on its driver tree, acted on rather than printed.
#
# Appium validates every driver in its manifest at startup and says so:
# `Driver "mac2" has 1 potential problem`. On charless-mac-mini that is a
# stale `mac2@1.20.5` beside a declared 3.7.0 server -- it never blocked THIS
# repair, because `uiautomator2` happened to install before it was reached,
# so nothing here would have noticed and the deadlock would have been waiting
# for whichever install came next. Reading the server's own complaint is not
# inference about npm's resolver; it is the declared authority on this tree
# stating that a driver disagrees with it, and the same conservative update
# answers it. Undeclared drivers are still only updated, never removed.
if [ -n "$appium_bin" ]; then
  verdict=$("$appium_bin" driver list --installed 2>&1 || printf '')
  case "$verdict" in
    *"potential problem"*)
      update_installed_drivers
      ;;
  esac
fi
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

/// Every driver the installed server itself calls incompatible with itself.
///
/// Read out of Appium's own startup validation, which writes
/// `Driver "mac2" (package `appium-mac2-driver`) may be incompatible with the
/// current version of Appium (v3.7.0) due to its peer dependency on Appium
/// ^2.4.1`. Using the server's verdict rather than comparing peer ranges here
/// keeps one authority on the question: the server is what will refuse to
/// host the driver, and a second opinion computed in Rust could disagree with
/// it.
pub fn incompatible_drivers(listing: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for line in listing.split("WARN") {
        if !line.contains("may be incompatible") {
            continue;
        }
        let Some(rest) = line.split_once("Driver \"") else {
            continue;
        };
        let Some((name, _)) = rest.1.split_once('"') else {
            continue;
        };
        if name.is_empty()
            || found
                .iter()
                .any(|(held, _): &(String, String)| held == name)
        {
            continue;
        }
        found.push((
            name.to_string(),
            line.split_whitespace().collect::<Vec<&str>>().join(" "),
        ));
    }
    found
}

/// The capability id this runtime implements, as
/// [`crate::capabilities::CAPABILITIES`] declares it.
pub const CAPABILITY_ID: &str = "mobile-app-capture";

/// The two mobile capture families, and the Appium driver each one cannot run
/// without.
///
/// The driver IS the routing rule, and that is deliberate: iOS capture is
/// XCUITest and Android capture is UiAutomator2, each a separate install, so
/// "may this host take this family" and "does this host declare that driver"
/// are the same question. Nothing here consults the host — a declaration is
/// what routing reads, for the reason `placement.rs` gives about a worker's
/// placement having lived in the registry AND in a file on the worker's disk
/// with only the file deciding.
pub const FAMILIES: &[(&str, &str)] = &[("ios", "xcuitest"), ("android", "uiautomator2")];

/// The driver one family requires, or `None` for a name that is not a mobile
/// capture family.
pub fn family_driver(family: &str) -> Option<&'static str> {
    FAMILIES
        .iter()
        .find(|(name, _)| *name == family)
        .map(|(_, driver)| *driver)
}

/// One host a mobile capture family may be placed on, with everything a
/// coordinator needs to run it there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    pub host: String,
    pub family: String,
    /// The Appium driver this family runs through, which the host declares.
    pub driver: String,
    /// Appium server version the host declares.
    pub appium: String,
    /// Absolute paths, in probe order, at which the coordinator resolves the
    /// Appium server. Never a bare program name: a non-interactive session on
    /// these hosts carries none of these directories on `PATH`, which is why
    /// `which adb` answers nothing on a host that has `adb`.
    pub appium_paths: Vec<String>,
    /// Absolute paths for `adb`, empty when the host declares no
    /// platform-tools.
    pub adb_paths: Vec<String>,
}

/// Every host the registry places one mobile capture family on, or every
/// placement when `family` is `None`.
///
/// A host that declares no `mobile_runtime`, or declares one without the
/// family's driver, is absent from the result and is therefore never probed:
/// asking it is what produced the finding that started this work, where a
/// refusal from a host that could not run the family at all was
/// indistinguishable from a fleet-wide policy gap.
pub fn placements(registry: &crate::targets::Registry, family: Option<&str>) -> Vec<Placement> {
    let appium_paths = declared_paths(super::host_exec::APPIUM_PROGRAM);
    let adb_paths = declared_paths(super::host_exec::ADB_PROGRAM);
    let mut found = Vec::new();
    for target in &registry.targets {
        let Some(declared) = requirement(target) else {
            continue;
        };
        for (name, driver) in FAMILIES {
            if family.is_some_and(|asked| asked != *name) {
                continue;
            }
            if !declared.drivers.iter().any(|held| held == *driver) {
                continue;
            }
            found.push(Placement {
                host: target.name.clone(),
                family: (*name).to_string(),
                driver: (*driver).to_string(),
                appium: declared.appium.clone(),
                appium_paths: appium_paths.clone(),
                adb_paths: if declared.platform_tools {
                    adb_paths.clone()
                } else {
                    Vec::new()
                },
            });
        }
    }
    found
}

/// One program's declared absolute paths, as a coordinator should read them.
///
/// `~/` is left as written: only the host knows its login home, and a
/// coordinator that expanded it here would be guessing about a machine it is
/// not running on.
fn declared_paths(program: &str) -> Vec<String> {
    super::host_exec::program_candidates(program)
        .unwrap_or(&[])
        .iter()
        .map(|path| (*path).to_string())
        .collect()
}

/// One program's declared absolute paths, rendered as shell words for the
/// remote scripts.
///
/// Taken from [`super::host_exec::program_candidates`] rather than written
/// out here, so the paths this module probes, installs into and reports are
/// the same paths the allowlist's own probe uses. Copying the list would have
/// let `stado host exec TARGET -- appium --version` and
/// `stado host mobile-runtime TARGET` disagree about which binary a host has.
///
/// A `~/`-anchored candidate becomes `"$HOME"/rest`, because only the host
/// knows what its login home is — the same expansion
/// [`super::host_exec`]'s own `home_anchored` performs, and for the same
/// reason.
pub fn candidate_words(program: &str) -> String {
    let Some(candidates) = super::host_exec::program_candidates(program) else {
        // A program with one path is that path. Quoted, so a candidate table
        // that ever grows a space cannot split into two words.
        return super::shlex_quote(program);
    };
    candidates
        .iter()
        .map(|candidate| match candidate.strip_prefix("~/") {
            Some(rest) => format!("\"$HOME\"/{}", super::shlex_quote(rest)),
            None => super::shlex_quote(candidate),
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// Fill a remote script's candidate placeholders from the shared table.
fn with_candidates(script: &str) -> String {
    script
        .replace(
            "@APPIUM_CANDIDATES@",
            &candidate_words(super::host_exec::APPIUM_PROGRAM),
        )
        .replace(
            "@ADB_CANDIDATES@",
            &candidate_words(super::host_exec::ADB_PROGRAM),
        )
        .replace(
            "@NODE_CANDIDATES@",
            &candidate_words(super::host_exec::NODE_PROGRAM),
        )
}

/// The version of one installed driver, out of `appium driver list
/// --installed`.
///
/// The listing is decorated differently by Appium 2 and 3 — bullets, colour
/// escapes, an `[installed (npm)]` suffix — and the one token both write
/// identically is `<name>@<version>`. Matched on the exact name so
/// `uiautomator2` is never read out of a line about a different driver, and
/// `None` when the driver is listed without a version rather than a guess.
pub fn installed_driver_version(listing: &str, driver: &str) -> Option<String> {
    let needle = format!("{driver}@");
    let mut rest = listing;
    while let Some(at) = rest.find(&needle) {
        // Reject a suffix match: `test@1` must not answer for `xcuitest@2`.
        let boundary_ok = rest[..at]
            .chars()
            .next_back()
            .is_none_or(|character| !character.is_ascii_alphanumeric() && character != '-');
        let tail = &rest[at + needle.len()..];
        let version: String = tail
            .chars()
            .take_while(|character| character.is_ascii_digit() || *character == '.')
            .collect();
        if boundary_ok && !version.is_empty() {
            return Some(version);
        }
        rest = tail;
    }
    None
}

/// Everything the host said about its runtime, judged against the
/// declaration.
pub async fn verify(
    target: &ComputeTarget,
    declared: &MobileRuntime,
    runner: &Runner,
) -> Result<RuntimeReport, DeployError> {
    let script = with_candidates(REMOTE_VERIFY_BODY);
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
        // The listing's columns differ between Appium 2 and 3, so this reads
        // the one shape both spell the same way: `<name>@<version>`. Reporting
        // the version and not just the name matters here — this host carried
        // an Appium 2-era driver set under a `$APPIUM_HOME` that outlived its
        // server, and "installed" alone would have called that agreement.
        let installed_version = installed_driver_version(&installed_drivers, driver);
        let present = installed_version.is_some()
            || installed_drivers
                .split(|character: char| !character.is_ascii_alphanumeric() && character != '-')
                .any(|word| word == driver.as_str());
        components.push(ComponentState {
            name: format!("driver:{driver}"),
            declared: "required".to_string(),
            path: "appium driver list --installed".to_string(),
            observed: installed_version.unwrap_or_else(|| {
                if present {
                    "installed".to_string()
                } else {
                    String::new()
                }
            }),
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

    // Visible, not judged. The server named these itself; a declaration that
    // says nothing about them cannot fail on them, and a report that omitted
    // them is how `charless-mac-mini` kept a driver the server refuses to
    // host, waiting to deadlock npm for the next install into that tree.
    for (driver, server_said) in incompatible_drivers(&field("warnings")) {
        if declared.drivers.contains(&driver) {
            continue;
        }
        let version = installed_driver_version(&installed_drivers, &driver)
            .unwrap_or_else(|| "installed".to_string());
        components.push(ComponentState {
            name: format!("driver:{driver}"),
            declared: "undeclared".to_string(),
            path: "appium driver list --installed".to_string(),
            // The server's own sentence, so an operator reads why rather than
            // a state name this module invented.
            observed: format!("{version} — {server_said}"),
            state: COMPONENT_UNDECLARED_INCOMPATIBLE.to_string(),
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
    let script = with_candidates(REMOTE_REPAIR_BODY)
        .replace("@APPIUM_B64@", &STANDARD.encode(declared.appium.as_bytes()))
        .replace(
            "@DRIVERS_B64@",
            &STANDARD.encode(declared.drivers.join(" ").as_bytes()),
        )
        .replace(
            "@PLATFORM_TOOLS_B64@",
            &STANDARD.encode(if declared.platform_tools { "yes" } else { "no" }),
        );
    let output =
        host_channel::run_script_with_timeout(target, &script, REPAIR_TIMEOUT, runner).await?;
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
    fn a_drivers_own_version_is_read_out_of_the_listing() {
        // The shape Appium 3 prints, bullets and suffix included.
        let listing = "- uiautomator2@5.0.1 [installed (npm)] - xcuitest@12.9.1 [installed (npm)]";
        assert_eq!(
            installed_driver_version(listing, "uiautomator2").as_deref(),
            Some("5.0.1")
        );
        assert_eq!(
            installed_driver_version(listing, "xcuitest").as_deref(),
            Some("12.9.1")
        );
        assert_eq!(installed_driver_version(listing, "mac2"), None);
    }

    #[test]
    fn a_driver_name_is_never_read_out_of_a_longer_one() {
        // `xcuitest@12.9.1` must not answer for a driver called `test`, which
        // is the bug a substring search would ship.
        let listing = "- xcuitest@12.9.1 [installed (npm)]";
        assert_eq!(installed_driver_version(listing, "test"), None);
    }

    #[test]
    fn a_listed_driver_with_no_version_is_not_guessed_at() {
        assert_eq!(
            installed_driver_version("- uiautomator2 [installed]", "uiautomator2"),
            None
        );
    }

    fn registry_with(entries: &[(&str, Value)]) -> crate::targets::Registry {
        let targets: Vec<Value> = entries
            .iter()
            .map(|(name, runtime)| {
                let mut target = serde_json::json!({"name": name, "kind": "local"});
                if !runtime.is_null() {
                    target
                        .as_object_mut()
                        .expect("an object")
                        .insert("mobile_runtime".to_string(), runtime.clone());
                }
                target
            })
            .collect();
        crate::targets::load_registry_from_str(
            &serde_json::json!({"schema_version": 2, "targets": targets}).to_string(),
        )
        .expect("a registry")
    }

    #[test]
    fn the_ios_family_routes_only_to_the_host_declaring_its_driver() {
        // The exact fleet shape: the mini carries Android only, the laptop
        // both, and a third host declares nothing at all.
        let registry = registry_with(&[
            (
                "charless-mac-mini",
                serde_json::json!({"appium":"3.7.0","drivers":["uiautomator2"],"platform_tools":true}),
            ),
            (
                "lukasz-macbook",
                serde_json::json!({"appium":"3.7.0","drivers":["xcuitest","uiautomator2"],"platform_tools":true}),
            ),
            ("ubuntu-server-rtx-pro-6000", Value::Null),
        ]);
        let ios: Vec<String> = placements(&registry, Some("ios"))
            .into_iter()
            .map(|placement| placement.host)
            .collect();
        assert_eq!(ios, vec!["lukasz-macbook".to_string()]);
    }

    #[test]
    fn the_android_family_routes_to_both_declared_hosts() {
        let registry = registry_with(&[
            (
                "charless-mac-mini",
                serde_json::json!({"appium":"3.7.0","drivers":["uiautomator2"],"platform_tools":true}),
            ),
            (
                "lukasz-macbook",
                serde_json::json!({"appium":"3.7.0","drivers":["xcuitest","uiautomator2"],"platform_tools":true}),
            ),
        ]);
        let android: Vec<String> = placements(&registry, Some("android"))
            .into_iter()
            .map(|placement| placement.host)
            .collect();
        assert_eq!(
            android,
            vec![
                "charless-mac-mini".to_string(),
                "lukasz-macbook".to_string()
            ]
        );
    }

    #[test]
    fn a_host_declaring_no_runtime_is_never_a_placement_for_any_family() {
        let registry = registry_with(&[("ubuntu-server-rtx-pro-6000", Value::Null)]);
        assert!(placements(&registry, None).is_empty());
    }

    #[test]
    fn a_placement_carries_absolute_paths_and_never_a_bare_program_name() {
        let registry = registry_with(&[(
            "lukasz-macbook",
            serde_json::json!({"appium":"3.7.0","drivers":["xcuitest"],"platform_tools":true}),
        )]);
        let placement = placements(&registry, Some("ios")).remove(0);
        assert!(!placement.appium_paths.is_empty());
        for path in placement
            .appium_paths
            .iter()
            .chain(placement.adb_paths.iter())
        {
            assert!(
                path.starts_with('/') || path.starts_with("~/"),
                "{path} is not an absolute or home-anchored path"
            );
        }
    }

    #[test]
    fn a_host_without_platform_tools_gets_no_adb_path_to_resolve() {
        let registry = registry_with(&[(
            "lukasz-macbook",
            serde_json::json!({"appium":"3.7.0","drivers":["xcuitest"],"platform_tools":false}),
        )]);
        assert!(placements(&registry, Some("ios"))
            .remove(0)
            .adb_paths
            .is_empty());
    }

    #[test]
    fn the_remote_scripts_resolve_the_paths_the_allowlist_probes() {
        // One table, two readers: the script the host runs must carry the same
        // candidates `host_exec` uses, or the two could disagree about which
        // binary a machine has.
        let script = with_candidates(REMOTE_VERIFY_BODY);
        assert!(!script.contains("@APPIUM_CANDIDATES@"));
        assert!(!script.contains("@ADB_CANDIDATES@"));
        assert!(!script.contains("@NODE_CANDIDATES@"));
        for candidate in
            crate::deploy::host_exec::program_candidates(crate::deploy::host_exec::APPIUM_PROGRAM)
                .expect("appium is in the table")
        {
            let expected = candidate
                .strip_prefix("~/")
                .map_or_else(|| (*candidate).to_string(), |rest| rest.to_string());
            assert!(script.contains(&expected), "{expected} missing from script");
        }
        // A home-relative candidate is expanded by the host, not here.
        assert!(script.contains("\"$HOME\"/"));
    }

    #[test]
    fn the_servers_own_incompatibility_warning_names_the_driver() {
        // Verbatim from charless-mac-mini.
        let listing = "WARN Appium Driver \"mac2\" has 1 potential problem: \n\
             WARN Appium   - Driver \"mac2\" (package `appium-mac2-driver`) may be incompatible \
             with the current version of Appium (v3.7.0) due to its peer dependency on Appium \
             ^2.4.1. Please install a compatible version of the driver.\n\
             - mac2@1.20.5 [installed (npm)]\n\
             - uiautomator2@8.5.2 [installed (npm)]";
        let found = incompatible_drivers(listing);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "mac2");
        assert!(found[0].1.contains("peer dependency"));
    }

    #[test]
    fn a_clean_listing_names_no_incompatible_driver() {
        assert!(incompatible_drivers("- uiautomator2@8.5.2 [installed (npm)]").is_empty());
    }

    #[test]
    fn an_undeclared_incompatible_driver_is_reported_and_does_not_fail_the_gate() {
        // The exact mini shape: everything declared is present, and the
        // server complains about a driver the declaration never mentions.
        let report = RuntimeReport {
            components: vec![
                component("appium", COMPONENT_PRESENT),
                component("driver:uiautomator2", COMPONENT_PRESENT),
                component("adb", COMPONENT_PRESENT),
                component("driver:mac2", COMPONENT_UNDECLARED_INCOMPATIBLE),
            ],
        };
        assert_eq!(report.verdict(), RUNTIME_COMPLETE);
        assert_eq!(report.failure("charless-mac-mini"), None);
        // Still visible: it is in the report an operator reads.
        assert!(report
            .components
            .iter()
            .any(|component| component.name == "driver:mac2"));
    }

    #[test]
    fn a_declared_driver_is_still_judged_even_beside_an_undeclared_one() {
        let report = RuntimeReport {
            components: vec![
                component("driver:uiautomator2", COMPONENT_MISSING),
                component("driver:mac2", COMPONENT_UNDECLARED_INCOMPATIBLE),
            ],
        };
        assert_eq!(report.verdict(), RUNTIME_INCOMPLETE);
        let said = report.failure("h").expect("a failure");
        assert!(said.contains("uiautomator2"));
        assert!(!said.contains("mac2"));
    }

    #[test]
    fn only_the_declared_families_are_routable() {
        assert_eq!(family_driver("ios"), Some("xcuitest"));
        assert_eq!(family_driver("android"), Some("uiautomator2"));
        assert_eq!(family_driver("windows"), None);
    }

    #[test]
    fn a_host_that_declares_nothing_is_not_judged() {
        let target: ComputeTarget =
            serde_json::from_value(serde_json::json!({"name":"h","kind":"local"}))
                .expect("a minimal target");
        assert!(requirement(&target).is_none());
    }
}
