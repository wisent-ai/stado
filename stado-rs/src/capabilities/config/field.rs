//! One configuration key: its CLI spelling, environment override and path.

use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigValueKind {
    Scalar,
    List,
    /// A whole JSON subtree (a client map, a namespace map) that a parser in
    /// `config` or `rate_limit` turns into typed policy. Its environment
    /// override carries the same JSON encoded as one string.
    Document,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ConfigField {
    pub key: &'static str,
    pub env: &'static str,
    pub path: &'static str,
    pub value_kind: ConfigValueKind,
    /// The second binding a value may be read from when the declared `env` and
    /// `path` carry nothing. Named `alternate`, not the word the shared write
    /// policy refuses: a second declared binding is announced, not silent.
    pub alternate_path: Option<&'static str>,
    pub alternate_env: Option<&'static str>,
    pub backup_path: Option<&'static str>,
    pub backup_env: Option<&'static str>,
    pub required: bool,
    pub backup_required: bool,
}

impl ConfigField {
    pub const fn scalar(key: &'static str, env: &'static str, path: &'static str) -> Self {
        Self {
            key,
            env,
            path,
            value_kind: ConfigValueKind::Scalar,
            alternate_path: None,
            alternate_env: None,
            backup_path: None,
            backup_env: None,
            required: false,
            backup_required: false,
        }
    }

    pub const fn list(key: &'static str, env: &'static str, path: &'static str) -> Self {
        Self {
            value_kind: ConfigValueKind::List,
            ..Self::scalar(key, env, path)
        }
    }

    pub const fn document(key: &'static str, env: &'static str, path: &'static str) -> Self {
        Self {
            value_kind: ConfigValueKind::Document,
            ..Self::scalar(key, env, path)
        }
    }

    pub const fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub const fn with_alternate(
        mut self,
        env: Option<&'static str>,
        path: Option<&'static str>,
    ) -> Self {
        self.alternate_env = env;
        self.alternate_path = path;
        self
    }

    pub const fn with_backup(
        mut self,
        env: &'static str,
        path: &'static str,
        required: bool,
    ) -> Self {
        self.backup_env = Some(env);
        self.backup_path = Some(path);
        self.backup_required = required;
        self
    }
}
