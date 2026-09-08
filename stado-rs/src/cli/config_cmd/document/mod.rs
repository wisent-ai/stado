//! `config init`, `config migrate` and `config validate`: the three verbs
//! that act on the configuration document as a whole rather than on one
//! dotted key of it. One component each: `init`, `migrate`, `validate`.

mod init;
mod migrate;
mod validate;

pub(super) use init::init;
pub(super) use migrate::migrate;
pub(super) use validate::validate;
