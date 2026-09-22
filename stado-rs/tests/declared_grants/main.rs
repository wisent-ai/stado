//! `stado service grants <SERVICE>` — the grants a service's consumers
//! declare, read from the registry rather than typed as flags.
//!
//! A consumer's grant existed only as `grant-sync` flags somebody remembered,
//! and 26 grants in the week of 2026-09-12 were issued from the shell instead,
//! one of them into the vault replica its owner overwrote within the hour.
//! Oko's judge named the missing product side on 2026-09-20: "deklaracja
//! konsumentów mintująca granty automatycznie z rejestru".
//!
//! Every case drives the built `stado` against a local storage backend under a
//! tempdir, with HOME and STADO_CONFIG isolated, so the operator's registry,
//! vault and cache are never read or written. Nothing here mints: minting
//! reaches a managed host, and what is checked here is the declaration the
//! minting reads and the refusals that keep it honest.


mod fixture;

#[path = "cases/reading.rs"]
mod reading;
#[path = "cases/writing.rs"]
mod writing;
