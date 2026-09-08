//! `stado dns` — the records of a zone Stado manages at its registrar.
//!
//! Namecheap has no per-record write. `namecheap.domains.dns.setHosts`
//! replaces the whole host list, so changing one name means re-sending every
//! other record in the zone, and a record left out of that call is deleted.
//! That is why `wisent.com`'s records were written by a script living inside a
//! product repository: the merge had to happen somewhere, and there was
//! nowhere in Stado for it.
//!
//! This is that place. Every command reads the whole zone, merges exactly one
//! name, and writes the whole zone back, so the merge is one implementation
//! the whole fleet shares.
//!
//! Two guards make a whole-zone rewrite safe to run. The parse is counted: if
//! the number of `<host` elements in the response does not equal the number of
//! records read out of it, the command refuses rather than writing a zone that
//! is missing whatever it failed to understand. And `EmailType=MX` travels
//! with every write, because a `setHosts` call without it can reset the mail
//! configuration of a zone that carries custom MX records — this zone carries
//! Google Workspace's.

mod command;
mod records;
mod registrar;
#[cfg(test)]
mod zone_merge_checks;

pub(crate) use self::command::{dispatch, DnsCommands};
pub(crate) use self::records::write::{ensure_record, remove_record};

const API: &str = "https://api.namecheap.com/xml.response";
const DEFAULT_CREDENTIAL: &str = "namecheap_auto";
const DEFAULT_TTL: &str = "1800";
const DEFAULT_MX_PREF: &str = "10";

/// The record types this plane writes. A zone carries more kinds than these,
/// and every one of them survives a merge untouched; the list bounds what a
/// Stado command will author, not what the zone may hold.
const WRITABLE_TYPES: &[&str] = &["A", "AAAA", "CNAME", "TXT", "ALIAS"];
