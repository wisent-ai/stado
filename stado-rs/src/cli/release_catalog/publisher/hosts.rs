//! The two hosts a publisher declaration or withdrawal is anchored to: the
//! vault owner and this host, both read from the registry; and declaring a
//! product's publisher between them.

use crate::cli::CmdError;

use super::declare_publisher;

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

/// Declare `product`'s publisher when this host's configuration does not,
/// so the first build or release of a new product publishes instead of
/// refusing with `release_api.publishers declares no publisher`.
///
/// That refusal used to be the only way a new product learned it needed
/// `catalog declare-publisher`, and the command needed a person to know which
/// host owns the vault. Both hosts are facts Stado can read: the client is
/// this host's registry target, and the owner is the host this vault
/// replicates, or this host when its vault is the authority. The declaration
/// itself is `declare_publisher`, unchanged.
pub(in crate::cli::release_catalog) async fn ensure_publisher(
    product: &str,
) -> Result<(), CmdError> {
    if crate::config::release_publisher_declared(product) {
        return Ok(());
    }
    // A publisher is the bearer a write to the release object API carries.
    // A store this process writes directly — the local backend with no API
    // configured, as an isolated release journey runs — takes no bearer, and
    // asking which host owns the fleet vault there refused every such run
    // with `no installed Skarbiec launcher` before its first write.
    if crate::config::stado_api_url().is_empty()
        && crate::capabilities::storage_adapter(crate::config::wc_storage_backend())
            == Some(crate::capabilities::StorageAdapter::Local)
    {
        return Ok(());
    }
    eprintln!("{product}: this host declares no release publisher for it; declaring it now");
    declare_publisher_on_fleet(product).await
}

/// `declare_publisher` between the fleet's vault owner and this host, with the
/// command it ran named in the refusal. Also used for a declared publisher
/// whose item Stado cannot read (`enroll::publishers`).
pub(in crate::cli::release_catalog) async fn declare_publisher_on_fleet(
    product: &str,
) -> Result<(), CmdError> {
    let (owner, client) = fleet_hosts().await?;
    eprintln!("{product}: declaring the publisher (vault owner {owner}, release client {client})");
    declare_publisher(product, &owner, &client, &[], &[], false)
        .await
        .map_err(|error| {
            CmdError::click(format!(
                "declaring {product}'s release publisher failed \
                 (stado release catalog declare-publisher {product} --owner {owner} \
                 --client {client}): {error}"
            ))
        })
}
