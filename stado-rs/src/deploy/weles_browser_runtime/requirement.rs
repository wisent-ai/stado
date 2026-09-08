//! What the installed release declares it needs: one pinned component, and
//! the parse of Playwright's own declaration of them.

use serde_json::Value;

use super::CACHE_ROOT;
use crate::deploy::DeployError;

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
