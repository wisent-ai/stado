//! A grant a service declares for Stado's own consumer: only reads, added to
//! the grant Stado's reads already use on the vault owner.

use crate::cli::CmdError;
use crate::targets::ConsumerGrant;

/// Add each declared `read:<item>#<field>` to the grant Stado's own reads
/// use, on the vault owner, keeping the bearer in the declared token file
/// there and every capability the grant holds. Only reads can be declared for
/// it; every capability is checked before any is added.
pub(super) async fn widen_stado_reads(grant: &ConsumerGrant) -> Result<String, CmdError> {
    let mut reads = Vec::new();
    for capability in &grant.capabilities {
        let read = capability
            .strip_prefix("read:")
            .and_then(|rest| rest.split_once('#'))
            .filter(|(item, field)| !item.is_empty() && !field.is_empty());
        let Some(read) = read else {
            return Err(CmdError::refused(format!(
                "{capability:?} cannot be added to Stado's own grant: only `read:<item>#<field>` \
                 is, so a declaration never widens what Stado may write"
            )));
        };
        reads.push(read);
    }
    let mut owner = String::new();
    for (item, field) in &reads {
        owner =
            crate::cli::host::ensure_declared_read(&grant.consumer, item, field, &grant.token_file)
                .await?;
    }
    let added: Vec<String> = reads
        .iter()
        .map(|(item, field)| format!("{item}#{field}"))
        .collect();
    Ok(format!(
        "{} may read {} on {owner}",
        grant.consumer,
        added.join(", ")
    ))
}
