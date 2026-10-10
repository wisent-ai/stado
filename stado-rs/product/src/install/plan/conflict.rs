//! A retained installation belongs to its receipt, not the next host requested.
use crate::{
    common::Runtime,
    state::{self, ProductState},
};
use anyhow::Result;
use std::{error::Error, fmt};

/// A lifecycle request would reuse an unfinished installation for another host.
/// The CLI classifies this type as refused input, not an unknown failure.
#[derive(Debug)]
pub struct InstallationHostConflict(String);

impl InstallationHostConflict {
    pub(super) fn recorded(
        runtime: &Runtime,
        installation: &ProductState,
        requested_host: Option<&str>,
    ) -> Result<Self> {
        let receipt = state::path(runtime, &installation.product, &installation.surface)?;
        let command = recovery_command(runtime, installation);
        let effect = if installation.surface == "service" && installation.previous.is_none() {
            "remove this incomplete surface and its managed service"
        } else {
            "roll back this incomplete installation to its retained predecessor"
        };
        Ok(Self(format!(
            "unfinished installation is bound to another host: id={:?}, surface={:?}, \
             host={:?}, started_at={:?}, receipt={:?}; requested_host={:?}. \
             On the machine holding this receipt, {effect} with `{command}`, then repeat \
             the requested installation. This recovery changes owned files and, for a \
             service, its recorded host's service; it is not run automatically. \
             Do not add --host to the recovery command: that would forward it to another \
             machine instead of resolving this receipt",
            installation.product,
            installation.surface,
            installation.host,
            installation.installed_at,
            receipt,
            requested_host,
        )))
    }
}

impl fmt::Display for InstallationHostConflict {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for InstallationHostConflict {}

fn recovery_command(runtime: &Runtime, installation: &ProductState) -> String {
    let mut command = format!(
        "env HOME={} stado product --workspace {}",
        quote(&runtime.home.to_string_lossy()),
        quote(&runtime.workspace.to_string_lossy()),
    );
    if !runtime.embedded_catalog {
        command.push_str(&format!(
            " --catalog {}",
            quote(&runtime.catalog.to_string_lossy())
        ));
    }
    let action = if installation.surface == "service" && installation.previous.is_none() {
        "remove"
    } else {
        "rollback"
    };
    command.push_str(&format!(
        " {action} {} --surface {}",
        quote(&installation.product),
        quote(&installation.surface),
    ));
    command
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
