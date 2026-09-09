//! The two optional registry sections the catalog is made of: the
//! `enrollment` allowances and the `channels` declarations, each parsed on
//! its own so an absent section keeps its documented default.

mod channels;
mod enrollment;

pub use channels::{parse_channels, ChannelsCatalog};
pub use enrollment::{parse_enrollment, EnrollmentCatalog};
