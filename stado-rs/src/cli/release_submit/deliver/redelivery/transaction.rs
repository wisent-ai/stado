//! Every durable boundary of one redelivery: create it, read it back, and
//! replace it only against the version the caller last observed.

use crate::cli::release_submit::deliver::redelivery::RedeliveryTransaction;
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;

pub(super) async fn load_redelivery_transaction(
    store: &JobStorage,
    path: &str,
) -> Result<Option<(RedeliveryTransaction, String)>, CmdError> {
    let Some(versioned) = store
        .read_text_versioned(path)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    else {
        return Ok(None);
    };
    Ok(Some((
        serde_json::from_str(&versioned.content)?,
        versioned.version,
    )))
}

pub(super) async fn create_redelivery_transaction(
    store: &JobStorage,
    path: &str,
    transaction: &RedeliveryTransaction,
) -> Result<(), CmdError> {
    let body = serde_json::to_string_pretty(transaction)?;
    if store
        .create_text_if_absent(path, &body)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
    {
        return Ok(());
    }
    Err(CmdError::click(
        "another redelivery transaction won the creation race; retry the command",
    ))
}

pub(super) async fn replace_redelivery_transaction(
    store: &JobStorage,
    path: &str,
    expected_version: &str,
    transaction: &RedeliveryTransaction,
) -> Result<(), CmdError> {
    store
        .compare_and_swap_text(
            path,
            expected_version,
            &serde_json::to_string_pretty(transaction)?,
        )
        .await
        .map_err(|error| {
            CmdError::click(format!(
                "redelivery transaction changed concurrently; retry the command: {error}"
            ))
        })?;
    Ok(())
}
