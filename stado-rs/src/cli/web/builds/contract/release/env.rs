//! The build-time variables the platform declares.

use std::path::Path;

use serde_json::Value;

use crate::cli::CmdError;

/// The build-time variables the platform declares in `.wisent-release.json`.
///
/// Read from the manifest rather than from the worker's environment because
/// the release worker passes `secret_env` and nothing else: a Skarbiec value
/// has to travel out of band, while a public constant is already in the file
/// this command reads for the product name. One document, two fields, one
/// reader.
pub(in crate::cli::web::builds) fn declared_env(
    source: &Path,
    platform: &str,
) -> Result<Vec<(String, String)>, CmdError> {
    let path = source.join(crate::release_pipeline::PRODUCT_MANIFEST);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    let declared: Value = serde_json::from_str(&text).map_err(|error| {
        CmdError::click(format!("{} is not valid JSON: {error}", path.display()))
    })?;
    let Some(Value::Object(entries)) = declared
        .get("platforms")
        .and_then(|platforms| platforms.get(platform))
        .and_then(|recipe| recipe.get("env"))
        .cloned()
    else {
        return Ok(Vec::new());
    };
    let mut pairs = Vec::new();
    for (name, value) in entries {
        let value = value.as_str().ok_or_else(|| {
            CmdError::click(format!(
                "{}: platform {platform} declares env.{name} as something other than a string",
                path.display()
            ))
        })?;
        pairs.push((name, value.to_string()));
    }
    Ok(pairs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_platforms_declared_env_is_read_from_the_release_manifest() {
        let directory =
            std::env::temp_dir().join(format!("stado-web-build-env-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join(crate::release_pipeline::PRODUCT_MANIFEST);
        std::fs::write(
            &path,
            serde_json::json!({
                "platforms": { "web": { "env": { "NEXT_PUBLIC_SITE_URL": "https://content.wisent.ai" } } }
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            declared_env(&directory, "web").unwrap(),
            vec![(
                "NEXT_PUBLIC_SITE_URL".to_string(),
                "https://content.wisent.ai".to_string()
            )]
        );
        // A platform that declares none, and a checkout with no manifest at
        // all, both mean no variables rather than a failure.
        assert!(declared_env(&directory, "linux-amd64").unwrap().is_empty());
        std::fs::remove_file(&path).unwrap();
        assert!(declared_env(&directory, "web").unwrap().is_empty());
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
