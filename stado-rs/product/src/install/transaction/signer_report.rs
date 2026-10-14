//! The actual identity of native bytes a source installation places.

use crate::{
    common::Runtime,
    install::plan::Placement,
    signing::{self, SignatureState},
    state::ProductState,
};
use anyhow::{bail, Result};
use serde_json::{json, Map, Value};

pub(super) struct Report {
    keychain: Option<String>,
    signatures: Map<String, Value>,
}

impl Report {
    pub(super) fn new(runtime: &Runtime, source_install: bool) -> Self {
        Self {
            keychain: if source_install {
                runtime.signing.keychain_path()
            } else {
                None
            },
            signatures: Map::new(),
        }
    }

    pub(super) fn observe(&mut self, placement: &Placement) -> Result<()> {
        if self.keychain.is_none() {
            return Ok(());
        }
        let bundle = placement.source.is_dir()
            && placement.source.extension().is_some_and(|ext| ext == "app");
        if !bundle && !signing::native(&placement.source)? {
            return Ok(());
        }
        let observed = signing::inspect(&placement.source)?;
        if !SignatureState::is_stable(&observed) {
            bail!(
                "{} was not signed with a stable Apple identity: {}",
                placement.source.display(),
                observed["error"]
            );
        }
        self.signatures.insert(
            placement.destination.to_string_lossy().into_owned(),
            json!({
                "identity": observed["authority"],
                "team": observed["team"],
                "identifier": observed["identifier"],
                "cdhash": observed["cdhash"],
            }),
        );
        Ok(())
    }

    pub(super) fn attach(self, state: &mut ProductState) {
        if let Some(keychain) = self.keychain {
            state.extra.insert(
                "signing".to_owned(),
                json!({"source": "local keychain", "keychain": keychain, "signatures": self.signatures}),
            );
        }
    }
}
