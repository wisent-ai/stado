//! The two hosts a publisher declaration or withdrawal is anchored to: the
//! vault owner and this host, both read from the registry.

use crate::cli::CmdError;

/// The vault owner and this host, as registry target names.
pub(crate) async fn fleet_hosts() -> Result<(String, String), CmdError> {
    let client = this_host().await?;
    let owner = vault_owner().await?;
    Ok((owner, client))
}

/// This host's registry target, as `stado resolver` identifies it.
pub(crate) async fn this_host() -> Result<String, CmdError> {
    let store = std::sync::Arc::new(crate::targets::RegistryStore::open().await?);
    let (bootstrap, _, _) = crate::cli::resolver::read_local_snapshot(&store).await?;
    crate::cli::resolver::current_target(&bootstrap).map_err(CmdError::declaration)
}

/// The host that owns the fleet vault: the registry's `skarbiec` active host,
/// the same answer `stado credentials vault show` gives. Reading it from this
/// machine's local vault file's replication bonds would name this host the
/// owner once its local copy is retired and replicates nothing; grants would
/// then be minted against a vault nobody reads ("<host> declares no vault
/// authority").
async fn vault_owner() -> Result<String, CmdError> {
    crate::cli::directory::active_host("skarbiec")
        .await?
        .ok_or_else(|| {
            CmdError::click(
                "cannot tell which host owns the vault: the service directory places no \
                 skarbiec active host",
            )
        })
}
