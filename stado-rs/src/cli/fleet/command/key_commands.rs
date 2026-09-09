//! The `stado fleet key` command tree: the published parser surface.

use clap::Subcommand;

#[derive(Subcommand)]
pub enum KeyCommands {
    /// Move an existing private key into the credential store (never printed).
    Add {
        /// Registry target the key belongs to.
        target: String,
        /// Private key file removed after verified storage.
        #[arg(long)]
        from: String,
    },
    /// List stored SSH host keys (metadata only).
    Ls,
    /// Remove a target's SSH key from the credential store.
    Rm {
        /// Registry target.
        target: String,
    },
    /// Install the stored public key into the target's authorized_keys.
    Install {
        /// Registry target.
        target: String,
    },
    /// Verify the stored key opens the channel to the target.
    Check {
        /// Registry target.
        target: String,
    },
    /// Generate a fresh ed25519 pair for the target into the credential store.
    Generate {
        /// Registry target.
        target: String,
    },
    /// Rotate the target's key end to end, with rollback on failure.
    Rotate {
        /// Registry target.
        target: String,
    },
}
