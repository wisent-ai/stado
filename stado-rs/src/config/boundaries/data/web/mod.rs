//! Web product boundary: declarations, validation and the declared edge.

mod edge;
mod product;
mod validate;

pub use edge::*;
pub use product::*;
pub use validate::*;

pub const WEB_API_EDGES: &[&str] = &["stado", "cloudflare"];
