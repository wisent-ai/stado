//! Which endpoint and which credential one client is built with.

use crate::cli::storage::*;

impl RemoteObjectApi {
    /// The configured object-API endpoint, or `None` when this process reads a
    /// disk-backed store directly.
    ///
    /// `STADO_API_URL` wins, but a deployment whose queue backend already IS
    /// the object API needs no second declaration: without this fallback,
    /// `fetch_object` handed an already-namespaced `storage_path()` to a
    /// backend that namespaces again, so every read resolved to
    /// `ecosystem/<ns>/ecosystem/<ns>/...` and answered "absent".
    fn endpoint_from_env_or_config() -> Result<Option<url::Url>, CmdError> {
        if let Some(url) = configured_object_base_url("STADO_API_URL")? {
            return Ok(Some(url));
        }
        if crate::capabilities::storage_adapter(crate::config::wc_storage_backend())
            != Some(crate::capabilities::StorageAdapter::StadoObject)
        {
            return Ok(None);
        }
        let configured = crate::config::wc_stado_storage_url();
        if configured.trim().is_empty() {
            return Ok(None);
        }
        url::Url::parse(configured.trim())
            .map(Some)
            .map_err(|error| CmdError::click(format!("storage.stado.url is not a URL: {error}")))
    }

    fn configured() -> Result<Option<Self>, CmdError> {
        let Some(base_url) = Self::endpoint_from_env_or_config()? else {
            return Ok(None);
        };
        let token = match std::env::var("STADO_API_TOKEN") {
            Ok(value) if !value.trim().is_empty() => value.trim().to_string(),
            Ok(_) | Err(std::env::VarError::NotPresent) => {
                let token_file = std::env::var("STADO_API_TOKEN_FILE")
                    .ok()
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| crate::config::wc_stado_storage_token_file().to_string());
                if token_file.trim().is_empty() {
                    return Err(CmdError::click(
                        "STADO_API_TOKEN, STADO_API_TOKEN_FILE or storage.stado.token_file \
                         is required to reach the object API",
                    ));
                }
                let path = crate::config_file::expand_tilde(token_file.trim());
                let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
                    CmdError::click(format!(
                        "cannot inspect STADO_API_TOKEN_FILE {}: {error}",
                        path.display()
                    ))
                })?;
                if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
                    return Err(CmdError::click(format!(
                        "STADO_API_TOKEN_FILE must be a regular file: {}",
                        path.display()
                    )));
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if metadata.permissions().mode() & 0o077 != 0 {
                        return Err(CmdError::click(format!(
                            "STADO_API_TOKEN_FILE must be owner-only (chmod 600): {}",
                            path.display()
                        )));
                    }
                }
                let value = std::fs::read_to_string(&path).map_err(|error| {
                    CmdError::click(format!(
                        "cannot read STADO_API_TOKEN_FILE {}: {error}",
                        path.display()
                    ))
                })?;
                let token = value.trim();
                if token.is_empty()
                    || token
                        .chars()
                        .any(|character| matches!(character, '\r' | '\n'))
                {
                    return Err(CmdError::click(
                        "STADO_API_TOKEN_FILE is empty or malformed",
                    ));
                }
                token.to_string()
            }
            Err(std::env::VarError::NotUnicode(_)) => {
                return Err(CmdError::click(
                    "STADO_API_TOKEN must be valid Unicode when STADO_API_URL is configured",
                ));
            }
        };
        let http = Self::http_client()?;
        Ok(Some(Self {
            http,
            base_url,
            auth: RemoteObjectAuth::Generic(token),
        }))
    }

    fn configured_with_auth(auth: RemoteObjectAuth) -> Result<Option<Self>, CmdError> {
        let Some(base_url) = Self::endpoint_from_env_or_config()? else {
            return Ok(None);
        };
        Ok(Some(Self {
            http: Self::http_client()?,
            base_url,
            auth,
        }))
    }

    pub(in crate::cli::storage) fn configured_release_reader() -> Result<Option<Self>, CmdError> {
        Self::configured_with_auth(RemoteObjectAuth::Public)
    }

    /// Release writes and exact release listings resolve their publisher bearer
    /// from Skarbiec. Constructing that path must not first demand the generic
    /// object credential it deliberately does not present.
    fn configured_release_writer() -> Result<Option<Self>, CmdError> {
        Self::configured_with_auth(RemoteObjectAuth::PublisherOnly)
    }

    pub(in crate::cli::storage) fn release_authorized(
        namespace: &str,
        key_or_prefix: &str,
    ) -> bool {
        matches!(namespace, "releases" | "sources")
            || (namespace == "system" && key_or_prefix.starts_with("release-catalog/"))
    }

    pub(in crate::cli::storage) fn configured_for_object(
        object: &crate::remote::object_store::ObjectRef,
    ) -> Result<Option<Self>, CmdError> {
        if Self::release_authorized(object.namespace(), object.key()) {
            Self::configured_release_writer()
        } else {
            Self::configured()
        }
    }

    pub(in crate::cli::storage) fn configured_for_list(
        namespace: &str,
        prefix: &str,
    ) -> Result<Option<Self>, CmdError> {
        if Self::release_authorized(namespace, prefix) {
            Self::configured_release_writer()
        } else {
            Self::configured()
        }
    }

    fn http_client() -> Result<reqwest::Client, CmdError> {
        fleet_https_client()
    }
}
