//! Data boundaries: databases, web products and integrations. Rate limits are
//! verified through Stado's Skarbiec identity.

mod database;
mod integration;
mod web;

pub use database::*;
pub use integration::*;
pub use web::*;
