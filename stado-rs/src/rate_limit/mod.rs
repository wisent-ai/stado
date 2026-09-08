//! Authenticated, provider-neutral fixed-window rate limiting.

mod auth;
mod consume;
mod error;
mod policy;

pub use auth::{authenticate, validate_verifier};
pub use consume::{ConsumeRequest, ConsumeResponse, RateLimiter};
pub use error::RateLimitError;
pub use policy::{clients, RateLimitClient};

pub(crate) use policy::parse_clients;
