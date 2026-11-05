//! `stado data`: the fleet's own measurements, published so that nothing in
//! them names a host, customer or job.

mod export;

use std::path::PathBuf;

use clap::Subcommand;

use crate::cli::CmdError;

#[derive(Subcommand)]
pub enum DataCommands {
    /// Write one kind of measurement as `<kind>.jsonl` and `manifest.json`
    /// into an empty or new directory: hosts and boards replaced by keyed
    /// pseudonyms, times kept to the hour, every other field either declared
    /// safe and kept or declared dropped. A field a record carries that its
    /// kind declares neither way is refused, naming it.
    Export {
        /// hours (per board and hour, from the beacons), capacity (each
        /// host's newest capacity publication) or prices (the kept market
        /// price readings).
        #[arg(long)]
        kind: String,
        /// Days back from now to export.
        #[arg(long = "since-days", default_value_t = 30)]
        since_days: u32,
        /// The directory the dataset is written into; it must be empty or
        /// absent.
        #[arg(long)]
        out: PathBuf,
        /// The credential role whose item holds the export key in field
        /// `key` (at least 32 characters). The same key gives the same
        /// pseudonyms in every export; another key gives unrelated ones.
        #[arg(long = "key-role")]
        key_role: String,
        #[arg(long)]
        json: bool,
    },
}

pub async fn dispatch(command: &DataCommands) -> Result<(), CmdError> {
    match command {
        DataCommands::Export {
            kind,
            since_days,
            out,
            key_role,
            json,
        } => export::export(kind, *since_days, out, key_role, *json).await,
    }
}
