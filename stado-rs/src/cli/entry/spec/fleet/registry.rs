//! The canonical compute-target registry: reading it, validating it, writing
//! it under a generation fence, and the hosts and connection paths it holds.

use clap::Subcommand;

#[derive(Subcommand)]
pub(crate) enum RegistryCommands {
    /// Validate a local registry-v2 JSON document.
    Validate { path: Option<String> },
    /// Additively adopt an existing registry-v2 JSON document.
    Import {
        /// Existing Stado registry-v2 JSON file.
        path: String,
        /// Emit a `stado.registry-import-receipt.v1` object.
        #[arg(long)]
        json: bool,
    },
    /// Upload local registry.json to the canonical registry object.
    ///
    /// With --if-generation the write is conditional on the generation the
    /// document was read at: a registry that has moved since is refused with
    /// exit 75 and, under --json, a `stado.registry-push-receipt.v1` object
    /// whose state is "conflict" and which names both generations. Exit 75
    /// means only that, so a reconcile loop can re-pull, re-apply and push
    /// again; a storage or validation failure stays exit 1.
    Push {
        /// The document to upload, or `-` to read it from stdin. With neither,
        /// the repository's bundled registry is uploaded - which is what
        /// erased the canonical document on 2026-09-01 when a caller piped a
        /// body this command never reads.
        path: Option<String>,
        /// Refuse the write unless the canonical registry is still at this
        /// generation. Take the token from `registry pull --generation-only`
        /// or `--with-generation`; a stale one exits 75.
        #[arg(long = "if-generation")]
        if_generation: Option<String>,
        /// Allow a write that deletes a top-level key the canonical document
        /// still carries. Without this the upload is refused and names them.
        /// It does NOT allow a write that erases every target.
        #[arg(long)]
        force: bool,
        /// Allow a write that leaves the registry with no targets at all.
        /// Separate from --force on purpose: every other guard asks whether a
        /// deletion was meant, and this one asks whether the document is a
        /// fleet at all.
        #[arg(long)]
        allow_empty_fleet: bool,
        /// Emit a `stado.registry-push-receipt.v1` object instead of the
        /// sentence, for both the write and the refusal.
        #[arg(long)]
        json: bool,
    },
    /// Print the canonical registry to stdout.
    ///
    /// Bare, this is the document alone. --with-generation prints one
    /// `stado.registry-pull-receipt.v1` object carrying the document and the
    /// token `push --if-generation` spends; --generation-only prints just the
    /// token. Both come from a single versioned read.
    Pull {
        /// Print the document and its generation as one typed receipt.
        #[arg(long, conflicts_with = "generation_only")]
        with_generation: bool,
        /// Print only the generation token, for a reconcile loop.
        #[arg(long)]
        generation_only: bool,
    },
    /// Print which registry target is this machine.
    #[command(name = "self")]
    SelfTarget {
        /// Print only the target name, for scripts.
        #[arg(long)]
        name_only: bool,
    },
    /// Diff registry declarations against live host state.
    Doctor {
        /// Emit the findings as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Manage hosts in the canonical registry.
    #[command(subcommand)]
    Host(RegistryHostCommands),
    /// Table of every registry host and its last beacon, worst first.
    #[command(name = "beacon-age")]
    BeaconAge {
        /// Emit the table as JSON.
        #[arg(long)]
        json: bool,
    },
}

fn parse_target_kind(raw: &str) -> Result<String, String> {
    crate::capabilities::configurable_variant(crate::capabilities::RuntimeFacet::HostTarget, raw)
        .map(|variant| variant.id.to_string())
        .ok_or_else(|| {
            let choices = crate::capabilities::configurable_ids(
                crate::capabilities::RuntimeFacet::HostTarget,
            )
            .collect::<Vec<_>>()
            .join(", ");
            format!("unknown target kind {raw:?}; use one of: {choices}")
        })
}

fn parse_release_platform(raw: &str) -> Result<String, String> {
    crate::deploy::products::managed_platform(raw)
        .map(str::to_string)
        .map_err(|error| error.to_string())
}

#[derive(Subcommand)]
pub(crate) enum RegistryHostCommands {
    /// Onboard HOST into the canonical registry, validated.
    Add {
        host: String,
        /// SSH destination ([user@]host[:port]) the fleet reaches HOST at.
        #[arg(long)]
        ssh: String,
        /// Registry target kind.
        #[arg(long, default_value = "local", value_parser = parse_target_kind)]
        kind: String,
        /// Release platform confirmed during enrollment.
        #[arg(long, value_parser = parse_release_platform)]
        release_platform: String,
    },
    /// Manage ordered SSH connection paths for an existing host.
    Path {
        #[command(subcommand)]
        command: RegistryHostPathCommands,
    },
}

#[derive(Subcommand)]
pub(crate) enum RegistryHostPathCommands {
    /// List the preferred path and ordered alternates.
    List {
        host: String,
        /// Emit the path list as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Add or replace one connection path.
    Set {
        host: String,
        /// Path identifier (`primary`, `nebula`, `tailscale`, `lan`, ...).
        path: String,
        /// SSH destination ([user@]host[:port]) for this path.
        #[arg(long)]
        ssh: String,
        /// Alternate priority starting at 1; omitted preserves its position or appends.
        #[arg(long)]
        priority: Option<usize>,
        /// Emit the mutation receipt as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Remove one alternate connection path.
    Remove {
        host: String,
        /// Alternate path identifier.
        path: String,
        /// Emit the mutation receipt as JSON.
        #[arg(long)]
        json: bool,
    },
}
