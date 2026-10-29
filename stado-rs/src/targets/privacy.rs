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
pub const PRIVACY_FOLDERS: &[(&str, &str)] = &[
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
    /// The grants this target declares, in declaration order; none when the
    /// target declares no `privacy_grants`. The registry was validated when
    /// it was loaded, so a present key has this shape; one that does not is
    /// a registry read past its validation and is refused naming the target.
    pub fn privacy_grants(&self) -> Result<Vec<PrivacyGrant>, RegistryValidationError> {
        let Some(value) = self.extra.get(PRIVACY_GRANTS_KEY) else {
            return Ok(Vec::new());
        };
        serde_json::from_value(value.clone()).map_err(|error| {
            RegistryValidationError(format!(
                "targets[{}].{PRIVACY_GRANTS_KEY} is not a list of grants: {error}",
                self.name
            ))
        })
    }
}

/// The folders `grants` declare for `program` on a host whose user home is
/// `home`: the consent-gated folders a walk that program runs may open.
/// macOS decides per program, so a folder declared for another program is
/// one this program must not open — opening it is the question nobody
/// declared. Paths are compared as written and as resolved, so a program
/// started through a link matches the grant naming its file.
pub fn folders_for(
    grants: &[PrivacyGrant],
    home: &Path,
    program: &Path,
) -> std::collections::BTreeSet<String> {
    let resolved = std::fs::canonicalize(program).ok();
    grants
        .iter()
        .filter(|grant| {
            let declared = grant.program_on(home);
            declared == program
                || resolved.as_ref().is_some_and(|resolved| {
                    std::fs::canonicalize(&declared).ok().as_ref() == Some(resolved)
                })
        })
        .map(|grant| grant.folder.clone())
        .collect()
}

/// [`folders_for`] the executable this process runs. A process that cannot
/// name its own executable is admitted into no folder.
pub fn folders_for_this_program(
    grants: &[PrivacyGrant],
    home: &Path,
) -> std::collections::BTreeSet<String> {
    match std::env::current_exe() {
        Ok(program) => folders_for(grants, home, &program),
        Err(_unnamed) => std::collections::BTreeSet::new(),
    }
}

/// The folders `grants` declare for any program on the host: what a walk
/// Stado sends to that host as a script (`find`, `du`) may open, since the
/// operator decided the fleet reads them there.
pub fn declared_folders(grants: &[PrivacyGrant]) -> std::collections::BTreeSet<String> {
    grants.iter().map(|grant| grant.folder.clone()).collect()
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
