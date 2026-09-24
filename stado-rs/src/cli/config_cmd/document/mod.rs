//! Whole-document configuration lifecycle and identity migration.

mod identities;
mod init;
mod migrate;
mod validate;

pub(super) use identities::migrate_identities;
pub(super) use init::init;
pub(super) use migrate::migrate;
pub(super) use validate::validate;
