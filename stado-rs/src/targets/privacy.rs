//! The protected folders a host's programs must be allowed to read: the
//! registry key `targets[].privacy_grants`.
//!
//! macOS decides per program whether it may open Documents, Desktop and
//! Downloads, and keeps that decision until it is changed in System Settings.
//! The decision itself is the person's at the keyboard (or a device-management
//! profile's); what the fleet owns is the statement of which program needs
//! which folder and why, so a host's measured access is judged against a
//! declaration instead of against whatever was clicked last.

use crate::targets::*;

/// The registry target key holding the declared grants.
pub const PRIVACY_GRANTS_KEY: &str = "privacy_grants";

/// The protected folders, by declaration key and the folder's name in a home.
pub const PRIVACY_FOLDERS: [(&str, &str); 3] = [
    ("documents", "Documents"),
    ("desktop", "Desktop"),
    ("downloads", "Downloads"),
];

/// One declared grant: PROGRAM must be allowed to read FOLDER, for REASON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivacyGrant {
    /// The executable macOS judges: absolute, or under `~/` (the host user's
    /// home).
    pub program: String,
    /// One of the [`PRIVACY_FOLDERS`] keys.
    pub folder: String,
    pub reason: String,
}

impl PrivacyGrant {
    /// The program's path on a host whose user home is `home`.
    pub fn program_on(&self, home: &Path) -> PathBuf {
        match self.program.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(&self.program),
        }
    }

    /// The refusal for a grant that cannot be declared, naming the field.
    pub fn defect(&self) -> Option<(&'static str, String)> {
        let program = self.program.as_str();
        if !(program.starts_with('/') || program.starts_with("~/")) || program.ends_with('/') {
            return Some((
                "program",
                "must be an executable's absolute path or a path under ~/".to_string(),
            ));
        }
        if !PRIVACY_FOLDERS.iter().any(|(key, _)| *key == self.folder) {
            let known: Vec<&str> = PRIVACY_FOLDERS.iter().map(|(key, _)| *key).collect();
            return Some(("folder", format!("must be one of {}", py_list_repr(&known))));
        }
        if self.reason.trim().is_empty() {
            return Some((
                "reason",
                "must say why the program needs the folder".to_string(),
            ));
        }
        None
    }
}

impl ComputeTarget {
    /// The grants this target declares, in declaration order. The registry
    /// was validated when it was loaded, so a present key has this shape.
    pub fn privacy_grants(&self) -> Vec<PrivacyGrant> {
        self.extra
            .get(PRIVACY_GRANTS_KEY)
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }
}

/// Check `targets[index].privacy_grants`: an array of `{program, folder,
/// reason}` on a darwin host, each grant well formed and no program and
/// folder declared twice.
pub(crate) fn validate_privacy_grants(
    location: &str,
    target: &Map<String, Value>,
    platform: &str,
) -> Result<(), RegistryValidationError> {
    let Some(value) = target.get(PRIVACY_GRANTS_KEY) else {
        return Ok(());
    };
    let location = format!("{location}.{PRIVACY_GRANTS_KEY}");
    let grants = value
        .as_array()
        .ok_or_else(|| verr(&location, "must be an array"))?;
    if !platform.starts_with("darwin") {
        return Err(verr(
            &location,
            "is allowed only on a darwin release_platform: only macOS decides folder access per program",
        ));
    }
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for (index, grant) in grants.iter().enumerate() {
        let at = format!("{location}[{index}]");
        let grant: PrivacyGrant = serde_json::from_value(grant.clone()).map_err(|error| {
            verr(
                &at,
                &format!("must be an object of program, folder and reason: {error}"),
            )
        })?;
        if let Some((field, defect)) = grant.defect() {
            return Err(verr(&format!("{at}.{field}"), &defect));
        }
        if !seen.insert((grant.program.clone(), grant.folder.clone())) {
            return Err(verr(
                &at,
                &format!(
                    "declares {} on {} a second time",
                    grant.program, grant.folder
                ),
            ));
        }
    }
    Ok(())
}
