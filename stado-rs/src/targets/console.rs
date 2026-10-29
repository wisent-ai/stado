//! What a host has in front of it: the registry key `targets[].console`.
//!
//! macOS asks the person at a Mac's screen before a program reaches the
//! local network or opens Documents, Desktop or Downloads, and the program
//! waits until somebody answers. Many fleet Macs have no display and nobody
//! logged in at them, so a question asked there is never answered. Whether
//! anybody can answer is a fact about the host, measured on it (`stado host
//! console record`): who owns `/dev/console` and how many display panels
//! (the built-in one, or a monitor that names itself) macOS reports online.
//! A Mac with no monitor still reports a display — the one macOS draws its
//! session into, or a display emulator's stand-in — which gives macOS no
//! name and is no panel. A host is `attended` only
//! when an account is logged in at the screen and a panel shows it; every
//! other host is `headless`, and on
//! a headless Mac Stado runs its own units as launchd daemons, which macOS
//! never asks about the local network, and declares no folder it would have
//! to ask about.

use crate::targets::*;

/// The registry target key holding the measured console.
pub const CONSOLE_KEY: &str = "console";

/// Whether a person can answer a question macOS shows on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleState {
    /// Nobody is logged in at the screen, or no display is online: a dialog
    /// shown there waits for an answer that never comes.
    Headless,
    /// An account is logged in at the screen and a display shows it.
    Attended,
}

impl ConsoleState {
    /// The state the evidence measures: attended only with both an account
    /// at the console and a display online.
    pub fn of(console_user: Option<&str>, displays: u32) -> Self {
        if console_user.is_some() && displays > 0 {
            Self::Attended
        } else {
            Self::Headless
        }
    }
}

impl std::fmt::Display for ConsoleState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        crate::primitives::word::write(self, formatter)
    }
}

/// One measurement of a host's console, as the registry records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleDeclaration {
    pub state: ConsoleState,
    /// The account that owns `/dev/console`, or `None` when the login window
    /// (root) holds it and nobody is logged in at the screen.
    pub console_user: Option<String>,
    /// The display panels macOS reported online: the built-in panel and
    /// attached monitors that name themselves.
    pub displays: u32,
    /// When the host was measured, RFC 3339.
    pub measured_at: String,
}

impl ConsoleDeclaration {
    /// The refusal for a declaration that is not a measurement, naming the
    /// field: a state its own evidence contradicts is a word somebody typed.
    pub fn defect(&self) -> Option<(&'static str, String)> {
        if self
            .console_user
            .as_deref()
            .is_some_and(|user| user.trim().is_empty())
        {
            return Some((
                "console_user",
                "must name the account that owns /dev/console, or be null".to_string(),
            ));
        }
        if chrono::DateTime::parse_from_rfc3339(&self.measured_at).is_err() {
            return Some((
                "measured_at",
                "must be the RFC 3339 time the host was measured".to_string(),
            ));
        }
        let measured = ConsoleState::of(self.console_user.as_deref(), self.displays);
        if self.state != measured {
            return Some((
                "state",
                format!(
                    "is {}, and its own evidence ({}) measures {measured}; record the host again \
                     with `stado host console record`",
                    self.state,
                    self.evidence()
                ),
            ));
        }
        None
    }

    /// The evidence in the words an operator reads.
    pub fn evidence(&self) -> String {
        let user = match self.console_user.as_deref() {
            Some(user) => format!("{user} is logged in at the screen"),
            None => "nobody is logged in at the screen".to_string(),
        };
        let displays = match self.displays {
            0 => "macOS reports no display panel online".to_string(),
            1 => "macOS reports 1 display panel online".to_string(),
            count => format!("macOS reports {count} display panels online"),
        };
        format!("{user} and {displays}")
    }
}

impl ComputeTarget {
    /// The console this target records; none when it records none. The
    /// registry was validated when it was loaded, so a present key has this
    /// shape; one that does not is refused naming the target.
    pub fn console(&self) -> Result<Option<ConsoleDeclaration>, RegistryValidationError> {
        let Some(value) = self.extra.get(CONSOLE_KEY) else {
            return Ok(None);
        };
        serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|error| {
                RegistryValidationError(format!(
                    "targets[{}].{CONSOLE_KEY} is not a console measurement: {error}",
                    self.name
                ))
            })
    }

    /// True when this target is a Mac whose recorded console is headless.
    /// A console that cannot be read is not headless: nothing is decided on
    /// a fact nobody read.
    pub fn declared_headless(&self) -> bool {
        self.release_platform.starts_with("darwin")
            && matches!(
                self.console(),
                Ok(Some(ConsoleDeclaration {
                    state: ConsoleState::Headless,
                    ..
                }))
            )
    }
}

/// The protected folder `path` lies in, by its name in a home, when it is
/// one: `~/<Folder>/...` or `/Users/<account>/<Folder>/...`.
pub fn protected_folder_of(path: &str) -> Option<&'static str> {
    let below_home = match path.strip_prefix("~/") {
        Some(rest) => rest,
        None => path
            .strip_prefix("/Users/")
            .and_then(|rest| rest.split_once('/'))
            .map(|(_, rest)| rest)?,
    };
    let first = below_home.split('/').next()?;
    PRIVACY_FOLDERS
        .iter()
        .find(|(_, folder)| *folder == first)
        .map(|(_, folder)| *folder)
}

/// Check `targets[index].console`: a measurement on a darwin host whose
/// state its evidence measures, and, on a headless host, no declaration
/// that needs somebody at the screen: no privacy grant and no work root in
/// a protected folder.
pub(crate) fn validate_console(
    location: &str,
    target: &Map<String, Value>,
    platform: &str,
) -> Result<(), RegistryValidationError> {
    let Some(value) = target.get(CONSOLE_KEY) else {
        return Ok(());
    };
    let at = format!("{location}.{CONSOLE_KEY}");
    if !platform.starts_with("darwin") {
        return Err(verr(
            &at,
            "is allowed only on a darwin release_platform: only macOS asks the person at the \
             screen before a program goes on",
        ));
    }
    let console: ConsoleDeclaration = serde_json::from_value(value.clone()).map_err(|error| {
        verr(
            &at,
            &format!(
                "must be the object `stado host console record` writes (state, console_user, \
                 displays, measured_at): {error}"
            ),
        )
    })?;
    if let Some((field, defect)) = console.defect() {
        return Err(verr(&format!("{at}.{field}"), &defect));
    }
    if console.state != ConsoleState::Headless {
        return Ok(());
    }
    if let Some(grants) = target
        .get(PRIVACY_GRANTS_KEY)
        .and_then(Value::as_array)
        .filter(|grants| !grants.is_empty())
    {
        let named: Vec<String> = grants
            .iter()
            .map(|grant| {
                format!(
                    "{} for {}",
                    grant["folder"].as_str().unwrap_or("an unnamed folder"),
                    grant["program"].as_str().unwrap_or("an unnamed program")
                )
            })
            .collect();
        return Err(verr(
            &format!("{location}.{PRIVACY_GRANTS_KEY}"),
            &format!(
                "declares {} on a host whose console is headless ({}): each grant is a question \
                 macOS asks the person at the screen, and nobody is there to answer it; remove \
                 the grants, or record the console again once someone is at the host",
                named.join(", "),
                console.evidence()
            ),
        ));
    }
    if let Some(root) = target.get("work_root").and_then(Value::as_str) {
        if let Some(folder) = protected_folder_of(root) {
            return Err(verr(
                &format!("{location}.work_root"),
                &format!(
                    "is {root}, inside {folder}, on a host whose console is headless ({}): \
                     macOS asks the person at the screen before a program opens {folder}, and \
                     nobody is there to answer; declare a work root outside Documents, Desktop \
                     and Downloads (`stado space root`)",
                    console.evidence()
                ),
            ));
        }
    }
    Ok(())
}
