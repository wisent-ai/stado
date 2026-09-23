//! Authentication keeps an explicitly provisioned resolver identity exclusive.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use russh::client;
use russh::keys::{self, agent::client::AgentClient, agent::AgentIdentity, PrivateKeyWithHashAlg};

use super::Peer;

pub(super) fn configured_identity(home: &Path) -> Option<PathBuf> {
    std::env::var("STADO_RESOLVER_SSH_KEY_FILE")
        .ok()
        .filter(|path| !path.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            let path = home.join(".stado/resolver-ssh-key");
            path.is_file().then_some(path)
        })
}

async fn file_identity(
    session: &mut client::Handle<Peer>,
    user: &str,
    path: &Path,
) -> Result<bool> {
    let key = match keys::load_secret_key(path, None) {
        Ok(key) => Arc::new(key),
        Err(keys::Error::KeyIsEncrypted) => {
            let public_path = PathBuf::from(format!("{}.pub", path.display()));
            let public = keys::load_public_key(&public_path)
                .with_context(|| format!("read public identity {} for SSH agent signing", public_path.display()))?;
            return agent_identities(session, user, Some(&public)).await;
        }
        Err(error) => return Err(error).with_context(|| format!("read SSH identity {}", path.display())),
    };
    let certificate = PathBuf::from(format!("{}-cert.pub", path.display()));
    if certificate.is_file() {
        let certificate = keys::load_openssh_certificate(&certificate)
            .with_context(|| format!("read SSH certificate {}", certificate.display()))?;
        if session
            .authenticate_openssh_cert(user, Arc::clone(&key), certificate)
            .await?
            .success()
        {
            return Ok(true);
        }
    }
    let hash = session.best_supported_rsa_hash().await?.flatten();
    Ok(session
        .authenticate_publickey(user, PrivateKeyWithHashAlg::new(key, hash))
        .await?
        .success())
}

pub(super) async fn authenticate(
    session: &mut client::Handle<Peer>,
    user: &str,
    home: &Path,
    identity: Option<&Path>,
) -> Result<()> {
    if let Some(path) = identity {
        if file_identity(session, user, path).await? {
            return Ok(());
        }
        bail!(
            "SSH identity {} was refused for {user}; no unrelated identity was offered",
            path.display()
        );
    }

    let mut failures = Vec::new();
    if std::env::var_os("SSH_AUTH_SOCK").is_some() {
        match agent_identities(session, user, None).await {
            Ok(true) => return Ok(()),
            Ok(false) => failures.push("SSH agent identities were refused".to_string()),
            Err(error) => failures.push(format!("SSH agent: {error:#}")),
        }
    }
    for name in ["id_ed25519", "id_ecdsa", "id_rsa"] {
        let path = home.join(".ssh").join(name);
        if !path.is_file() {
            continue;
        }
        match file_identity(session, user, &path).await {
            Ok(true) => return Ok(()),
            Ok(false) => failures.push(format!("{} was refused", path.display())),
            Err(error) => failures.push(format!("{error:#}")),
        }
    }
    bail!(
        "SSH authentication failed for {user}: {}; provision an authorized SSH identity",
        if failures.is_empty() { "no identity is available".to_string() } else { failures.join("; ") }
    )
}

async fn agent_identities(
    session: &mut client::Handle<Peer>,
    user: &str,
    required: Option<&keys::PublicKey>,
) -> Result<bool> {
    let mut agent = AgentClient::connect_env().await?;
    let identities = agent.request_identities().await?;
    let hash = session.best_supported_rsa_hash().await?.flatten();
    for identity in identities {
        if required.is_some_and(|key| key.key_data() != identity.public_key().key_data()) {
            continue;
        }
        let result = match identity {
            AgentIdentity::PublicKey { key, .. } => {
                session.authenticate_publickey_with(user, key, hash, &mut agent).await?
            }
            AgentIdentity::Certificate { certificate, .. } => {
                session.authenticate_certificate_with(user, certificate, hash, &mut agent).await?
            }
        };
        if result.success() {
            return Ok(true);
        }
    }
    Ok(false)
}
