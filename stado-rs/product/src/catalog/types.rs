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
