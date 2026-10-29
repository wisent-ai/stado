//! What a Mac has in front of it, measured on the host through its declared
//! connection: who owns `/dev/console` and which displays macOS reports
//! online. The measurement is what `targets[].console` records
//! ([`crate::targets::ConsoleDeclaration`]); nothing here opens a window,
//! asks for a permission, or changes the host.

use serde_json::Value;

use crate::deploy::{host_channel, DeployError, Runner};
use crate::targets::{ComputeTarget, ConsoleDeclaration, ConsoleState};

const SCRIPT: &str = r#"set -u
os=$(/usr/bin/uname -s)
if [ "$os" != Darwin ]; then
  printf 'STADO_CONSOLE_UNSUPPORTED\t%s\n' "$os"
  exit 0
fi
flat() { printf '%s' "$1" | /usr/bin/tr '\t\r\n' '   '; }
if owner=$(/usr/bin/stat -f%Su /dev/console 2>&1); then
  printf 'STADO_CONSOLE_OWNER\t%s\n' "$(printf '%s' "$owner" | /usr/bin/tr -d ' \t\r\n')"
else
  printf 'STADO_CONSOLE_FAILED\t%s\n' "$(flat "stat -f%Su /dev/console: $owner")"
fi
if displays=$(/usr/sbin/system_profiler -json SPDisplaysDataType 2>/dev/null); then
  printf 'STADO_CONSOLE_DISPLAYS\t%s\n' "$(flat "$displays")"
else
  printf 'STADO_CONSOLE_FAILED\t%s\n' "system_profiler -json SPDisplaysDataType exited $?"
fi
printf 'STADO_CONSOLE_END\n'
"#;

/// The accounts that own `/dev/console` while nobody is logged in at the
/// screen: the login window runs as root.
const NOBODY: &[&str] = &["root", "loginwindow"];

/// One measurement of a Mac's console.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Measured {
    /// What owns `/dev/console`, as `stat` printed it.
    pub owner: String,
    /// The names of the online displays a person can look at.
    pub displays: Vec<String>,
    /// Every display `system_profiler` reported online, as it reported it,
    /// counted or not: the evidence behind `displays`.
    pub reported: Vec<Value>,
    /// The measurement as the registry records it.
    pub declaration: ConsoleDeclaration,
}

/// The displays of every graphics processor that `system_profiler -json
/// SPDisplaysDataType` reports online (`spdisplays_online` other than
/// `spdisplays_no`).
fn online_displays(document: &Value) -> Vec<Value> {
    document["SPDisplaysDataType"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|gpu| gpu["spdisplays_ndrvs"].as_array().into_iter().flatten())
        .filter(|display| display["spdisplays_online"].as_str() != Some("spdisplays_no"))
        .cloned()
        .collect()
}

/// The `_name` `system_profiler` gives a display whose EDID carries no
/// product name: its untranslated placeholder.
const UNNAMED_DISPLAY: &str = "spdisplays_display";

/// Whether an online display is a panel a person can look at: the built-in
/// one, or an attached monitor that names itself in its EDID. A Mac with no
/// monitor attached still reports an online display — the one macOS draws
/// its session into, or the stand-in a display emulator plugged into its
/// HDMI port answers with — and that one gives macOS no product name.
fn is_panel(display: &Value) -> bool {
    display["spdisplays_connection_type"].as_str() == Some("spdisplays_internal")
        || display["_name"]
            .as_str()
            .is_some_and(|name| !name.is_empty() && name != UNNAMED_DISPLAY)
}

/// Measure `target`'s console.
pub async fn measure(target: &ComputeTarget, runner: &Runner) -> Result<Measured, DeployError> {
    let output = host_channel::run_script(target, SCRIPT, runner).await?;
    let name = &target.name;
    let mut owner = None;
    let mut displays = None;
    let mut failures = Vec::new();
    let mut ended = false;
    for line in output.stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_CONSOLE_UNSUPPORTED", os] => {
                return Err(DeployError(format!(
                    "{name} runs {os}; a console is measured only on a Mac, where macOS asks \
                     the person at the screen before a program goes on"
                ))
                .stating(crate::primitives::failure::FailureCode::Refused))
            }
            ["STADO_CONSOLE_OWNER", value] => owner = Some(value.to_string()),
            ["STADO_CONSOLE_DISPLAYS", value] => displays = Some(value.to_string()),
            ["STADO_CONSOLE_FAILED", said] => failures.push(said.to_string()),
            ["STADO_CONSOLE_END"] => ended = true,
            _ => {}
        }
    }
    if !ended {
        return Err(DeployError::unreachable(format!(
            "{name}: the console read ended without its end line (exit {}): {}",
            output.code,
            host_channel::last_error_line(&output, "the host said nothing")
        )));
    }
    let (Some(owner), Some(displays)) = (owner, displays) else {
        return Err(DeployError::unreachable(format!(
            "{name}: the console was not measured: {}",
            failures.join("; ")
        )));
    };
    let document: Value = serde_json::from_str(&displays).map_err(|error| {
        DeployError::unreachable(format!(
            "{name}: system_profiler -json SPDisplaysDataType answered no JSON ({error})"
        ))
    })?;
    let reported = online_displays(&document);
    let displays: Vec<String> = reported
        .iter()
        .filter(|display| is_panel(display))
        .map(|display| match display["_name"].as_str() {
            Some(name) => name.to_string(),
            None => "an unnamed display".to_string(),
        })
        .collect();
    let console_user =
        (!owner.is_empty() && !NOBODY.contains(&owner.as_str())).then(|| owner.clone());
    let count = u32::try_from(displays.len()).unwrap_or(u32::MAX);
    let declaration = ConsoleDeclaration {
        state: ConsoleState::of(console_user.as_deref(), count),
        console_user,
        displays: count,
        measured_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    };
    Ok(Measured {
        owner,
        displays,
        reported,
        declaration,
    })
}

/// Whether the recorded console and a fresh measurement say the same thing:
/// the state, the account at the screen and the number of displays. The
/// time of measurement is not compared.
pub fn agrees(recorded: &ConsoleDeclaration, measured: &ConsoleDeclaration) -> bool {
    recorded.state == measured.state
        && recorded.console_user == measured.console_user
        && recorded.displays == measured.displays
}
