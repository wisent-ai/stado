//! A fresh SSH key pair, generated in a unique transient path whose files are
//! removed when the pair has been read.

use crate::cli::CmdError;
use crate::deploy::{CommandSpec, Runner};
use crate::primitives::failure::FailureCode;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use super::super::run_checked;

pub(super) struct KeyPair {
    pub(super) private_key: String,
    pub(super) public_key: String,
    pub(super) fingerprint: String,
}

struct GeneratedFiles {
    private: PathBuf,
    public: PathBuf,
}

impl Drop for GeneratedFiles {
    // A transient private key left behind is an unencrypted key on disk, so a
    // removal that fails is said with the path rather than ignored.
    fn drop(&mut self) {
        for path in [&self.private, &self.public] {
            if let Err(error) = std::fs::remove_file(path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    eprintln!(
                        "stado fleet key: the transient key {} could not be removed and is still on disk: {error}",
                        path.display()
                    );
                }
            }
        }
    }
}

/// Generate an SSH key pair in a unique transient path guarded by Drop.
pub(super) async fn generate_pair(runner: &Runner, comment: &str) -> Result<KeyPair, CmdError> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            CmdError::declaration(format!(
                "the system clock reads before the Unix epoch: {error}"
            ))
        })?
        .as_nanos();
    let path =
        std::env::temp_dir().join(format!("stado-fleet-keygen-{}-{nonce}", std::process::id()));
    let path_str = path
        .to_str()
        .ok_or_else(|| {
            CmdError::declaration(format!(
                "the transient key path {} is not text ssh-keygen can be handed",
                path.display()
            ))
        })?
        .to_string();
    run_checked(
        runner,
        CommandSpec::new(vec![
            "ssh-keygen".to_string(),
            "-t".to_string(),
            "ed25519".to_string(),
            "-f".to_string(),
            path_str.clone(),
            "-N".to_string(),
            String::new(),
            "-C".to_string(),
            comment.to_string(),
        ]),
        "ssh-keygen ed25519",
        FailureCode::InfraDown,
    )
    .await?;
    let files = GeneratedFiles {
        private: path,
        public: PathBuf::from(format!("{path_str}.pub")),
    };
    let read = |path: &PathBuf| {
        std::fs::read_to_string(path).map_err(|exc| {
            CmdError::from(exc).within(format!("cannot read the generated key {}", path.display()))
        })
    };
    let private_key = read(&files.private)?;
    let public_key = read(&files.public)?;
    let fingerprint_line = run_checked(
        runner,
        CommandSpec::new(vec!["ssh-keygen".to_string(), "-lf".to_string(), path_str]),
        "ssh-keygen -lf",
        FailureCode::InfraDown,
    )
    .await?;
    let fingerprint = fingerprint_line
        .split_whitespace()
        .find(|part| part.starts_with("SHA256:"))
        .ok_or_else(|| {
            CmdError::click(format!(
                "ssh-keygen -lf printed no SHA256 fingerprint for the generated key: {fingerprint_line}"
            ))
        })?
        .to_string();
    Ok(KeyPair {
        private_key: private_key.trim().to_string(),
        public_key: public_key.trim().to_string(),
        fingerprint,
    })
}
