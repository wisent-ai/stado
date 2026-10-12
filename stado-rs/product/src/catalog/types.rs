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

#[derive(Deserialize)]
pub struct RoadmapItem<'a> {
    pub title: &'a str,
    pub status: &'a str,
    pub outcome: &'a str,
    pub source: &'a str,
}

#[derive(Deserialize)]
pub struct Integration<'a> {
    pub product: &'a str,
    pub description: &'a str,
    pub source: &'a str,
}

/// How an installation recipe builds or fetches its surface. The catalogue's
/// `kind` word is parsed into this once; each kind declares the fields its
/// recipe carries, so validation, `registry` editing and installation read
/// one table instead of three.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "stado-release" => Self::StadoRelease,
            "desktop-release" => Self::DesktopRelease,
            "local-build" => Self::LocalBuild,
            "cargo" => Self::Cargo,
            "npm" => Self::Npm,
            "pipx" => Self::Pipx,
            "pip" => Self::Pip,
            _ => return None,
        })
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

/// Whether a recipe field holds a comma-separated list.
pub fn list_field(field: &str) -> bool {
    field == BINARIES || field == ENTRYPOINTS || field == FEATURES
}
