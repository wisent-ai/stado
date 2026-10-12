//! Web product boundary: declarations, validation and the declared edge.

mod edge;
mod product;
mod validate;

pub use edge::*;
pub use product::*;
pub use validate::*;

/// The edge that answers a web product's hostname.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    serde::Serialize,
    serde::Deserialize,
    clap::ValueEnum,
)]
#[serde(rename_all = "lowercase")]
#[value(rename_all = "lowercase")]
pub enum WebEdge {
    /// Stado's own edge: the declared `web_api.edge` host serves the record.
    #[default]
    Stado,
    /// A Cloudflare tunnel publishes the hostname.
    Cloudflare,
}

impl std::fmt::Display for WebEdge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match serde_json::to_value(self) {
            Ok(serde_json::Value::String(word)) => formatter.write_str(&word),
            _ => Err(std::fmt::Error),
        }
    }
}
