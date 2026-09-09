//! What the deployment env file declares: the keys naming one product's
//! staged release, the coordinate they carry, and the refusals for a
//! declaration that is absent or is not a digest.

use super::*;

/// One product's coordinate, as the deployment env file declares it.
#[derive(Debug, PartialEq, Eq)]
pub struct Coordinate {
    pub version: String,
    pub sha256: String,
    pub local_root: String,
}

/// The env keys naming one product's staged release.
pub fn coordinate_keys(product: &str) -> (String, String) {
    let stem = product.replace('-', "_").to_uppercase();
    (
        format!("{stem}_RELEASE_VERSION"),
        format!("{stem}_RELEASE_SHA256"),
    )
}

/// Read one product's coordinate out of a deployment env file.
///
/// Last assignment wins, because the file is sourced top to bottom.
pub fn coordinate(body: &str, product: &str) -> Result<Coordinate, DeployError> {
    let (version_key, sha_key) = coordinate_keys(product);
    let mut version = None;
    let mut sha256 = None;
    let mut local_root = None;
    for line in body.lines() {
        let trimmed = line.trim_start();
        let assignment = trimmed
            .strip_prefix("export ")
            .map_or(trimmed, str::trim_start);
        for (key, slot) in [
            (&version_key, &mut version),
            (&sha_key, &mut sha256),
            (&LOCAL_ROOT_KEY.to_string(), &mut local_root),
        ] {
            if let Some(value) = assignment.strip_prefix(&format!("{key}=")) {
                *slot = Some(service_env_file::effective_text(value).trim().to_string());
            }
        }
    }
    let missing = |key: &str| {
        DeployError(format!(
            "the deployment env file declares no {key}; add it to the deployment env file \
             before activating a staged release"
        ))
    };
    let coordinate = Coordinate {
        version: version
            .filter(|v| !v.is_empty())
            .ok_or_else(|| missing(&version_key))?,
        sha256: sha256
            .filter(|v| !v.is_empty())
            .ok_or_else(|| missing(&sha_key))?,
        local_root: local_root
            .filter(|v| !v.is_empty())
            .ok_or_else(|| missing(LOCAL_ROOT_KEY))?,
    };
    if !coordinate
        .sha256
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit())
        || coordinate.sha256.len() != 64
    {
        return Err(DeployError(format!(
            "the deployment env file declares {sha_key}={:?}, which is not a sha256 digest; \
             replace it with the staged archive's 64-character digest",
            coordinate.sha256
        )));
    }
    Ok(coordinate)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENV_FILE: &str = "export STADO_RELEASE_LOCAL_ROOT=$HOME/.stado/releases\n\
                            WELES_WORKER_RELEASE_VERSION=0.5.21\n\
                            # a later assignment wins, as a sourced file would\n\
                            WELES_WORKER_RELEASE_VERSION=\"0.5.43\"\n\
                            WELES_WORKER_RELEASE_SHA256=2714720eea1eaa430000000000000000000000000000000000000000000000ab\n";

    #[test]
    fn the_coordinate_is_the_last_assignment_the_env_file_makes() {
        let read = coordinate(ENV_FILE, "weles-worker").unwrap();
        assert_eq!(read.version, "0.5.43");
        assert_eq!(read.local_root, "$HOME/.stado/releases");
        assert_eq!(
            archive_path(&read, "weles-worker", "darwin-arm64"),
            "$HOME/.stado/releases/weles-worker/0.5.43/darwin-arm64/weles-worker.tar.gz"
        );
    }

    #[test]
    fn an_env_file_naming_no_staged_release_is_refused_by_key() {
        let said = coordinate("STADO_RELEASE_LOCAL_ROOT=/r\n", "weles-worker")
            .unwrap_err()
            .to_string();
        assert!(said.contains("WELES_WORKER_RELEASE_VERSION"), "{said}");
        let said = coordinate(
            "STADO_RELEASE_LOCAL_ROOT=/r\nWELES_WORKER_RELEASE_VERSION=0.5.43\n\
             WELES_WORKER_RELEASE_SHA256=nothex\n",
            "weles-worker",
        )
        .unwrap_err()
        .to_string();
        assert!(said.contains("is not a sha256 digest"), "{said}");
    }
}
