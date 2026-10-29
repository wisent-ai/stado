//! This host's network identity: an X25519 key pair generated on the host
//! and kept in its Stado home, owner-only. Only the public half leaves the
//! host, in the registry; the private half is never carried anywhere, which
//! is why it is minted here rather than in a vault the network must first
//! reach.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

use base64::Engine;
use serde::{Deserialize, Serialize};

/// The directory every network file of this account lives in.
pub fn directory() -> Result<PathBuf, String> {
    Ok(crate::config_file::home()
        .map_err(|cause| format!("the Stado network keeps no files without a home: {cause}"))?
        .join(".stado")
        .join("network"))
}

/// Where this account's key pair is kept.
pub fn path() -> Result<PathBuf, String> {
    Ok(directory()?.join("key"))
}

/// One node's static key pair.
#[derive(Clone)]
pub struct Keypair {
    pub(crate) private: Vec<u8>,
    pub(crate) public: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    private: String,
    public: String,
}

impl Keypair {
    /// The public key as the registry writes it.
    pub fn public_base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(&self.public)
    }
}

/// Decode a registry public key.
pub fn decode_public(text: &str) -> Result<Vec<u8>, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|error| format!("public key {text:?} is not base64: {error}"))?;
    if bytes.len() != 32 {
        return Err(format!(
            "public key {text:?} is {} bytes; an X25519 key is 32",
            bytes.len()
        ));
    }
    Ok(bytes)
}

/// This account's key pair, refused naming the file and the command that
/// creates it when there is none.
pub fn load() -> Result<Keypair, String> {
    let path = path()?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!(
                "this account has no Stado network key at {}; `stado network up` creates it",
                path.display()
            ))
        }
        Err(error) => return Err(format!("read {}: {error}", path.display())),
    };
    let stored: Stored = serde_json::from_str(&text)
        .map_err(|error| format!("{} is not a Stado network key: {error}", path.display()))?;
    let engine = base64::engine::general_purpose::STANDARD;
    let private = engine
        .decode(&stored.private)
        .map_err(|error| format!("{}: private key is not base64: {error}", path.display()))?;
    let public =
        decode_public(&stored.public).map_err(|cause| format!("{}: {cause}", path.display()))?;
    Ok(Keypair { private, public })
}

/// The key pair, generated and written owner-only when this account has
/// none. The flag says whether it was created now.
pub fn load_or_create() -> Result<(Keypair, bool), String> {
    let path = path()?;
    if path.exists() {
        return load().map(|keypair| (keypair, false));
    }
    let params = super::NOISE_PARAMS
        .parse()
        .map_err(|error| format!("Noise parameters {}: {error}", super::NOISE_PARAMS))?;
    let generated = snow::Builder::new(params)
        .generate_keypair()
        .map_err(|error| format!("generate a network key pair: {error}"))?;
    let keypair = Keypair {
        private: generated.private,
        public: generated.public,
    };
    let engine = base64::engine::general_purpose::STANDARD;
    let text = serde_json::to_string(&Stored {
        private: engine.encode(&keypair.private),
        public: keypair.public_base64(),
    })
    .map_err(|error| format!("encode the network key: {error}"))?;
    let directory = directory()?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&directory)
        .map_err(|error| format!("create {}: {error}", directory.display()))?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("restrict {}: {error}", directory.display()))?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    Ok((keypair, true))
}
