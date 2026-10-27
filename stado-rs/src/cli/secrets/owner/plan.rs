//! What an owner move would do, read before anything changes: the placement
//! profile that carries Skarbiec, the host that owns the vault and the one it
//! moves to, who owns it now, whether the new owner is registered and whose
//! keyring holds its secret half, and the live items the move must keep.

use serde_json::{json, Value};

use super::remote::{live_items, VaultHost};
use crate::cli::CmdError;
use crate::deploy::Runner;
use crate::placement::{PlacementProfile, StateRoot};

/// The logical service whose vault moves.
pub(crate) const SERVICE: &str = "skarbiec";

/// One recipient as the vault registers it.
pub(super) struct Recipient {
    pub uid: String,
    pub fingerprint: String,
}

pub(super) struct Plan {
    pub profile: PlacementProfile,
    pub source: VaultHost,
    pub destination: VaultHost,
    /// The vault's path under each host's home, as the profile carries it.
    pub relative: String,
    pub owner_uid: String,
    pub previous: Recipient,
    /// The new owner as the vault registers it, `None` while unregistered.
    pub registered: Option<Recipient>,
    /// Why the destination's keyring holds no secret half for the new
    /// owner, `None` when it holds one.
    pub missing_secret: Option<String>,
    pub live: Vec<String>,
}

impl Plan {
    /// Every fact the move depends on, from one registry read: the profile,
    /// the directory's owner host, both hosts' entries and their Skarbiec
    /// releases. Each further read of the registry is a round trip to its
    /// authority, so it is read here and handed down.
    pub(super) async fn read(to: &str, owner_uid: &str, runner: &Runner) -> Result<Self, CmdError> {
        let (document, _) = crate::cli::registry::fetch_versioned_document().await?;
        let registry =
            crate::targets::load_registry_from_value(&document).map_err(CmdError::from)?;
        let profile = vault_profile(&document)?;
        let source_name = crate::cli::directory::active_host_in(&document, SERVICE)?
            .ok_or_else(|| {
                CmdError::declaration(format!(
                    "the service directory names no active host for {SERVICE}, so no host owns a vault to move"
                ))
            })?;
        let source_target = crate::cli::resolved_host(&registry, &source_name)?.clone();
        let destination_target = crate::cli::resolved_host(&registry, to)?.clone();
        let destination_name = destination_target.name.clone();
        if destination_name == source_target.name {
            return Err(CmdError::refused(format!(
                "{destination_name} already holds the fleet vault"
            )));
        }
        if !profile.hosts.contains_key(&destination_name) {
            return Err(CmdError::refused(format!(
                "{destination_name} is not a host of placement profile {}; prepare it with `stado placement standby {} --host {destination_name}` first",
                profile.name, profile.name
            )));
        }
        let source = VaultHost::owner(source_target, &document, runner).await?;
        let relative = vault_state(&profile, &source)?;
        let destination =
            VaultHost::destination(destination_target, &relative, &document, runner).await?;
        let recipients = source
            .skarbiec(&["recipient", "list"], None, runner)
            .await
            .map_err(CmdError::refused)?;
        let previous = owner_of(&recipients)?;
        let registered = recipient(&recipients, owner_uid)?;
        let missing_secret = destination
            .gpg(&["--list-secret-keys", "--", owner_uid], None, runner)
            .await
            .err();
        let listed = source
            .skarbiec(&["list"], None, runner)
            .await
            .map_err(CmdError::refused)?;
        let live = live_items(&listed).map_err(CmdError::declaration)?;
        Ok(Self {
            profile,
            source,
            destination,
            relative,
            owner_uid: owner_uid.to_string(),
            previous,
            registered,
            missing_secret,
            live,
        })
    }

    /// What makes a missing owner key on the destination: the applied move
    /// runs Skarbiec's `owner key mint` there, which makes exactly that key
    /// with the parameters `init` uses and nothing else.
    pub(super) fn mint_hint(&self) -> String {
        format!(
            "--apply makes it on {} with `skarbiec owner key mint {}`",
            self.destination.target.name, self.owner_uid
        )
    }

    pub(super) fn report(&self) -> Value {
        json!({
            "profile": self.profile.name,
            "services": self.profile.services,
            "from_host": self.source.target.name,
            "to_host": self.destination.target.name,
            "vault": self.relative,
            "previous_owner": self.previous.uid,
            "owner_uid": self.owner_uid,
            "rotation_required": self.previous.uid != self.owner_uid,
            "owner_registered": self.registered.is_some(),
            "destination_secret_key": match &self.missing_secret {
                None => Value::from("present"),
                Some(cause) => Value::from(format!("absent: {cause}; {}", self.mint_hint())),
            },
            "live_items": self.live.len(),
        })
    }

    /// Check the exact files placement will require before changing the owner.
    /// The placement transaction checks them again to catch intervening changes.
    pub(super) async fn check_destination(&self, runner: &Runner) -> Result<(), CmdError> {
        let host = &self.destination.target;
        let declared = self.profile.hosts.get(&host.name).ok_or_else(|| {
            CmdError::declaration(format!(
                "{} has no placement host {}",
                self.profile.name, host.name
            ))
        })?;
        for service in &self.profile.services {
            let unit = declared.units.get(service).ok_or_else(|| {
                CmdError::declaration(format!(
                    "{} declares no destination unit for {service}",
                    self.profile.name
                ))
            })?;
            let crate::placement::PlacementLifecycle::Managed(managed) = &unit.lifecycle else {
                return Err(CmdError::refused(format!(
                    "{}: {service} is release-controlled, not a managed unit this vault move can relocate",
                    host.name
                )));
            };
            let exists = crate::deploy::host_channel::remote_test(
                host,
                &format!("-f {}", crate::deploy::shlex_quote(&managed.path)),
                runner,
            )
            .await
            .map_err(CmdError::from)?;
            if !exists {
                return Err(CmdError::missing(format!(
                    "{}: destination unit file is missing: {}; the vault owner was not changed; prepare it with stado placement standby {} --host {} --reason REASON",
                    host.name, managed.path, self.profile.name, host.name
                )));
            }
        }
        Ok(())
    }
}

/// The one placement profile whose services include Skarbiec.
fn vault_profile(document: &Value) -> Result<PlacementProfile, CmdError> {
    let mut carrying: Vec<PlacementProfile> = crate::placement::profiles(document)
        .map_err(CmdError::declaration)?
        .into_iter()
        .filter(|profile| profile.services.iter().any(|service| service == SERVICE))
        .collect();
    let Some(profile) = carrying.pop() else {
        return Err(CmdError::declaration(format!(
            "no placement profile carries {SERVICE}; declare one before its vault can move"
        )));
    };
    if !carrying.is_empty() {
        carrying.push(profile);
        return Err(CmdError::declaration(format!(
            "placement profiles {} all carry {SERVICE}, so which one moves its vault cannot be told",
            carrying
                .iter()
                .map(|profile| profile.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    Ok(profile)
}

/// The vault's path under the owner's home, refused unless the profile
/// carries exactly that path as required home state: a vault the move does
/// not carry would be rotated and left behind.
fn vault_state(profile: &PlacementProfile, source: &VaultHost) -> Result<String, CmdError> {
    let relative = source
        .vault
        .strip_prefix(&format!("{}/", source.home))
        .ok_or_else(|| {
            CmdError::declaration(format!(
                "{}'s vault {} is not under its home {}, so where the move writes it on another host cannot be told",
                source.target.name, source.vault, source.home
            ))
        })?;
    let carried = profile
        .state
        .iter()
        .any(|state| state.path == relative && state.required && state.root == StateRoot::Home);
    if !carried {
        return Err(CmdError::declaration(format!(
            "placement profile {} does not carry {relative} as required home state, so a move would leave the vault behind",
            profile.name
        )));
    }
    Ok(relative.to_string())
}

/// The recipient the vault registers as its owner.
fn owner_of(recipients: &Value) -> Result<Recipient, CmdError> {
    let registered = recipients.as_object().ok_or_else(|| {
        CmdError::declaration(format!(
            "skarbiec recipient list answered no map: {recipients}"
        ))
    })?;
    let mut owners: Vec<&String> = registered
        .iter()
        .filter(|(_, entry)| entry.get("role").and_then(Value::as_str) == Some("owner"))
        .map(|(uid, _)| uid)
        .collect();
    let Some(uid) = owners.pop() else {
        return Err(CmdError::declaration(format!(
            "the vault registers no owner: {recipients}"
        )));
    };
    if !owners.is_empty() {
        return Err(CmdError::declaration(format!(
            "the vault registers more than one owner: {recipients}"
        )));
    }
    recipient(recipients, uid)?.ok_or_else(|| {
        CmdError::declaration(format!("the vault's owner {uid} has no registry entry"))
    })
}

/// `uid` as the vault registers it, `None` when it does not.
fn recipient(recipients: &Value, uid: &str) -> Result<Option<Recipient>, CmdError> {
    let Some(entry) = recipients.get(uid) else {
        return Ok(None);
    };
    let fingerprint = entry
        .get("fingerprint")
        .and_then(Value::as_str)
        .filter(|fingerprint| !fingerprint.is_empty())
        .ok_or_else(|| {
            CmdError::declaration(format!(
                "the vault registers {uid} without a fingerprint: {entry}"
            ))
        })?;
    Ok(Some(Recipient {
        uid: uid.to_string(),
        fingerprint: fingerprint.to_string(),
    }))
}
