//! Declarative local-inference deployments managed through the canonical
//! Stado registry. Runtime lifecycle lives in `deploy::inference`; this module
//! owns the stable document and plan contracts.

pub mod plan;
pub mod reservation;
pub mod schema;

#[cfg(test)]
mod tests;
