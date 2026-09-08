//! The host operations that are not the reconcile pass: the probe that answers
//! before anything is installed, the status of a session that already runs, the
//! pairing a client asks for, and the stop that ends it.

mod pair;
mod probe;
mod status;
mod stop;

pub use pair::pair;
pub use probe::{bus_id_for, probe, xorg_bus_id};
pub use status::status;
pub use stop::stop;
