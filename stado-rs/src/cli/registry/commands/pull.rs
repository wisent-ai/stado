//! `stado registry pull` — print the canonical document, and the token that
//! makes it writable back, from one versioned read.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cli::CmdError;
use crate::targets::RegistryStore;

/// `stado registry pull --with-generation`: the document and the token that
/// makes it writable back, from one read.
///
/// Two reads cannot produce this object safely — the generation would belong
/// to a different document than the one printed beside it, which is the exact
/// lost update `--if-generation` exists to refuse.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryPullReceipt {
    schema: String,
    location: String,
    generation: String,
    document: Value,
}

const PULL_RECEIPT_SCHEMA: &str = "stado.registry-pull-receipt.v1";

/// `stado registry pull [--with-generation | --generation-only]` — print the
/// canonical registry.
///
/// Bare, it prints the pretty document and nothing else: scripts pipe this
/// into `jq`. `--with-generation` prints one
/// `stado.registry-pull-receipt.v1` object carrying the document and the
/// token `push --if-generation` spends, and `--generation-only` prints just
/// the token. Both come from ONE versioned read, because a generation read
/// separately from the document it is supposed to describe is a token for a
/// document nobody looked at.
pub async fn pull(with_generation: bool, generation_only: bool) -> Result<(), CmdError> {
    let store = RegistryStore::open().await?;
    let blob = store.read_versioned().await?.ok_or_else(|| {
        CmdError::click(format!(
            "could not fetch registry from {}",
            store.location()
        ))
    })?;
    if generation_only {
        println!("{}", blob.version);
        return Ok(());
    }
    let value: Value = serde_json::from_str(&blob.content)?;
    if with_generation {
        println!(
            "{}",
            serde_json::to_string_pretty(&RegistryPullReceipt {
                schema: PULL_RECEIPT_SCHEMA.to_string(),
                location: store.location().to_string(),
                generation: blob.version,
                document: value,
            })?
        );
        return Ok(());
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
