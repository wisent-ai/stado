use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SurfaceKind {
    Cli,
    Desktop,
    Service,
    #[serde(other)]
    Other,
}

impl SurfaceKind {
    pub fn needs_recipe(&self) -> bool {
        matches!(self, Self::Cli | Self::Desktop | Self::Service)
    }
}

/// Where a roadmap item stands; any other word fails to deserialize.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoadmapStatus {
    Planned,
    InProgress,
}

#[derive(Deserialize)]
pub struct RoadmapItem<'a> {
    pub title: &'a str,
    /// Validated by deserialization; nothing else reads it.
    #[allow(dead_code)]
    pub status: RoadmapStatus,
    pub outcome: &'a str,
    pub source: &'a str,
}

/// Where a catalog product stands; any other word fails to deserialize.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProductStatus {
    Active,
    Preview,
    Retired,
}

/// Who may see a product's repositories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Public,
    Private,
}

/// Whether a product is part of the Wisent family or stands alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Family {
    Wisent,
    Standalone,
}

impl std::fmt::Display for Family {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        crate::word::write(self, formatter)
    }
}

#[derive(Deserialize)]
pub struct Integration<'a> {
    pub product: &'a str,
    pub description: &'a str,
    pub source: &'a str,
}

/// One product another product needs to run, and the way it runs without it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    /// The product needed.
    pub product: String,
    /// Its surface that is needed.
    pub surface: String,
    /// The surface of this product that needs it; every surface when absent.
    #[serde(default)]
    pub for_surface: Option<String>,
    /// How this product runs without it, in the popular solution it falls to;
    /// absent when it does not run without it.
    #[serde(default)]
    pub alternative: Option<String>,
    /// The code that implements the alternative, as `REPOSITORY/PATH[:LINES]`
    /// relative to the workspace of checkouts.
    #[serde(default)]
    pub alternative_source: Option<String>,
    /// Why this product does not run without it, when that is by design: a
    /// surface of the other product and not a separate product, or work that
    /// is the other product's by definition. Declared instead of an
    /// alternative, never beside one; `stado product dependencies` reports
    /// the need as bound by this reason instead of as a dependency nobody
    /// decided on.
    #[serde(default)]
    pub bound_because: Option<String>,
}

/// How an installation recipe builds or fetches its surface. The catalogue's
/// `kind` word is parsed into this once; each kind declares the fields its
/// recipe carries, so validation, `registry` editing and installation read
/// one table instead of three.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RecipeKind {
    StadoRelease,
    DesktopRelease,
    LocalBuild,
    Cargo,
    Npm,
    Pipx,
    Pip,
}

impl RecipeKind {
    /// The kind `word` names; a word that names none is serde's refusal,
    /// which lists the kinds there are.
    pub fn parse(word: &str) -> serde_json::Result<Self> {
        Self::deserialize(serde_json::Value::String(word.to_string()))
    }

    /// The field a bare `registry` installation value fills; every recipe of
    /// this kind must carry it.
    pub fn primary(self) -> &'static str {
        match self {
            Self::StadoRelease | Self::DesktopRelease | Self::Cargo => MANIFEST,
            Self::LocalBuild => COMMAND,
            Self::Npm | Self::Pipx => BINARIES,
            Self::Pip => PACKAGE,
        }
    }

    /// The second field a recipe of this kind must carry, if it has one.
    pub fn also_required(self) -> Option<&'static str> {
        match self {
            Self::Cargo => Some(BINARIES),
            Self::Pip => Some(ENTRYPOINTS),
            _ => None,
        }
    }

    /// A field a recipe of this kind may carry without being required.
    pub fn optional(self) -> Option<&'static str> {
        match self {
            Self::Cargo => Some(FEATURES),
            _ => None,
        }
    }

    /// Whether a recipe of this kind may carry `field`.
    pub fn allows(self, field: &str) -> bool {
        field == self.primary()
            || self.also_required() == Some(field)
            || self.optional() == Some(field)
            || field == SIGNING_KEYCHAIN
    }

    /// Installed by a language package manager into an isolated prefix.
    pub fn packaged(self) -> bool {
        matches!(self, Self::Cargo | Self::Npm | Self::Pipx | Self::Pip)
    }

    /// The field listing the commands an installed recipe exposes.
    pub fn commands_field(self) -> &'static str {
        match self {
            Self::Pip => ENTRYPOINTS,
            _ => BINARIES,
        }
    }
}

pub const MANIFEST: &str = "manifest";
pub const COMMAND: &str = "command";
pub const BINARIES: &str = "binaries";
pub const PACKAGE: &str = "package";
pub const ENTRYPOINTS: &str = "entrypoints";
pub const FEATURES: &str = "features";
pub const SIGNING_KEYCHAIN: &str = "signing_keychain";

/// Whether a recipe field holds a comma-separated list.
pub fn list_field(field: &str) -> bool {
    field == BINARIES || field == ENTRYPOINTS || field == FEATURES
}

/// The local keychain a recipe's `signing_keychain` names: an absolute path,
/// or `$HOME/<path>` inside the installing account's home, never with `..`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclaredKeychain<'a> {
    Absolute(&'a std::path::Path),
    Home(&'a std::path::Path),
}

impl<'a> DeclaredKeychain<'a> {
    /// The keychain `named` declares, refused naming the rule it breaks.
    pub fn parse(named: &'a str) -> anyhow::Result<Self> {
        use std::path::{Component, Path};
        let declared = match named.strip_prefix("$HOME/") {
            Some(relative) => Self::Home(Path::new(relative)),
            None => Self::Absolute(Path::new(named)),
        };
        let (path, placed) = match declared {
            Self::Absolute(path) => (path, path.is_absolute()),
            Self::Home(path) => (path, !path.is_absolute() && !path.as_os_str().is_empty()),
        };
        if !placed
            || path
                .components()
                .any(|part| matches!(part, Component::ParentDir))
        {
            anyhow::bail!(
                "{SIGNING_KEYCHAIN} {named:?}: name an absolute local keychain or $HOME/<path> without .."
            );
        }
        Ok(declared)
    }

    /// The keychain's path on a machine whose account home is `home`.
    pub fn under(self, home: &std::path::Path) -> std::path::PathBuf {
        match self {
            Self::Absolute(path) => path.to_path_buf(),
            Self::Home(path) => home.join(path),
        }
    }
}
