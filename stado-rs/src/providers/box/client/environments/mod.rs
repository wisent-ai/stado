//! Boat environments: what every new sandbox started with one gets —
//! environment variables, secret files and GitHub repositories. Every change
//! mints a new immutable version; a sandbox already running stays on its
//! version until it is upgraded. A change answers `success=true` rather
//! than `ok` and a type, so it goes through `request_change`.

use serde_json::{json, Map, Value};

use super::super::types::BoxError;
use super::files::segment_path;
use super::BoxClient;

/// The path of one environment by its id.
fn environment_path(environment: &str) -> Result<String, BoxError> {
    segment_path("/environments", environment, "an environment id")
}

/// The path of one variable of one environment.
fn variable_path(environment: &str, key: &str) -> Result<String, BoxError> {
    segment_path(
        &format!("{}/vars", environment_path(environment)?),
        key,
        "an environment variable's key",
    )
}

impl BoxClient {
    /// GET /environments.
    pub async fn environments(&self) -> Result<Map<String, Value>, BoxError> {
        self.transport
            .request_json("GET", "/environments", None, &[], &["environments.list"])
            .await
    }

    /// POST /environments: a new, empty environment named `name`; Boat
    /// answers the whole list with it.
    pub async fn create_environment(&self, name: &str) -> Result<Map<String, Value>, BoxError> {
        if name.is_empty() {
            return Err(BoxError::value("a Boat environment needs a name"));
        }
        let body = json!({ "name": name });
        self.transport
            .request_json(
                "POST",
                "/environments",
                Some(&body),
                &[],
                &["environments.list"],
            )
            .await
    }

    /// DELETE /environments/{environment}.
    pub async fn delete_environment(
        &self,
        environment: &str,
    ) -> Result<Map<String, Value>, BoxError> {
        self.transport
            .request_change("DELETE", &environment_path(environment)?, None, &[])
            .await
    }

    /// PUT /environments/{environment}/vars/{key}: set one variable.
    pub async fn set_environment_variable(
        &self,
        environment: &str,
        key: &str,
        value: &str,
    ) -> Result<Map<String, Value>, BoxError> {
        let body = json!({ "value": value });
        self.transport
            .request_change("PUT", &variable_path(environment, key)?, Some(&body), &[])
            .await
    }

    /// DELETE /environments/{environment}/vars/{key}.
    pub async fn remove_environment_variable(
        &self,
        environment: &str,
        key: &str,
    ) -> Result<Map<String, Value>, BoxError> {
        self.transport
            .request_change("DELETE", &variable_path(environment, key)?, None, &[])
            .await
    }

    /// PUT /environments/{environment}/secret-files: write the secret file at
    /// `path` in every new sandbox with `contents`.
    pub async fn set_environment_secret_file(
        &self,
        environment: &str,
        path: &str,
        contents: &str,
    ) -> Result<Map<String, Value>, BoxError> {
        if path.is_empty() {
            return Err(BoxError::value("a Boat secret file needs a path"));
        }
        let body = json!({ "path": path, "contents": contents });
        self.transport
            .request_change(
                "PUT",
                &format!("{}/secret-files", environment_path(environment)?),
                Some(&body),
                &[],
            )
            .await
    }

    /// DELETE /environments/{environment}/secret-files?path=.
    pub async fn remove_environment_secret_file(
        &self,
        environment: &str,
        path: &str,
    ) -> Result<Map<String, Value>, BoxError> {
        if path.is_empty() {
            return Err(BoxError::value("a Boat secret file needs a path"));
        }
        self.transport
            .request_change(
                "DELETE",
                &format!("{}/secret-files", environment_path(environment)?),
                None,
                &[("path", path.to_string())],
            )
            .await
    }

    /// POST /environments/{environment}/repos: clone the GitHub repository
    /// whose id Boat's repository list gives into every new sandbox, on
    /// `base_branch` when one is named.
    pub async fn add_environment_repository(
        &self,
        environment: &str,
        repository: &str,
        base_branch: Option<&str>,
    ) -> Result<Map<String, Value>, BoxError> {
        if repository.is_empty() {
            return Err(BoxError::value(
                "a Boat environment repository needs its id",
            ));
        }
        let mut body = json!({ "repositoryId": repository });
        if let Some(branch) = base_branch {
            body["baseBranch"] = Value::from(branch);
        }
        self.transport
            .request_change(
                "POST",
                &format!("{}/repos", environment_path(environment)?),
                Some(&body),
                &[],
            )
            .await
    }

    /// GET /repos: the GitHub repositories Boat can clone, grouped by
    /// installation, with each one's `databaseId` that `repository` above
    /// takes, and the ones already selected. `query` filters by name, `sync`
    /// reads GitHub again first, and an empty `cursor` asks for the first
    /// page.
    pub async fn repositories(
        &self,
        query: &str,
        sync: bool,
        cursor: &str,
    ) -> Result<Map<String, Value>, BoxError> {
        let mut parameters = vec![("q", query.to_string()), ("cursor", cursor.to_string())];
        if sync {
            parameters.push(("sync", sync.to_string()));
        }
        self.transport
            .request_json("GET", "/repos", None, &parameters, &["repos.list"])
            .await
    }
}
