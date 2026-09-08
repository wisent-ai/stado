//! How the registrar is addressed: one second-level label, one top-level
//! label, and the host label a fully qualified name reduces to inside them.
//!
//! It sits under the registrar because the shape is the registrar's own —
//! `SLD` and `TLD` are two separate parameters of every Namecheap call — and
//! because a name that is not inside the zone has to be refused before any
//! credential is read.

use crate::cli::CmdError;

/// A zone split the way Namecheap addresses it.
pub(in crate::cli::dns) struct Zone {
    pub(in crate::cli::dns) name: String,
    pub(in crate::cli::dns) sld: String,
    pub(in crate::cli::dns) tld: String,
}

impl Zone {
    pub(in crate::cli::dns) fn parse(zone: &str) -> Result<Self, CmdError> {
        let zone = zone.trim().trim_end_matches('.').to_ascii_lowercase();
        let Some((sld, tld)) = zone.rsplit_once('.') else {
            return Err(CmdError::usage(format!(
                "{zone:?} is not a zone name; a zone is at least two labels, for example wisent.com"
            )));
        };
        if sld.contains('.') {
            return Err(CmdError::usage(format!(
                "{zone:?} names more than one zone level; \
                 Namecheap addresses a zone as one second-level and one top-level label"
            )));
        }
        if sld.is_empty() || tld.is_empty() {
            return Err(CmdError::usage(format!("{zone:?} is not a zone name")));
        }
        Ok(Self {
            name: zone.clone(),
            sld: sld.to_string(),
            tld: tld.to_string(),
        })
    }

    /// The zone a fully qualified name belongs to, when the caller does not
    /// name one: the last two labels.
    pub(in crate::cli::dns) fn of(name: &str) -> Result<Self, CmdError> {
        let name = name.trim().trim_end_matches('.').to_ascii_lowercase();
        let labels: Vec<&str> = name.split('.').collect();
        if labels.len() < 2 {
            return Err(CmdError::usage(format!(
                "{name:?} is not a fully qualified name"
            )));
        }
        Zone::parse(&labels[labels.len() - 2..].join("."))
    }

    /// The host label `setHosts` wants for one fully qualified name: `@` for
    /// the apex, otherwise everything to the left of the zone.
    pub(in crate::cli::dns) fn host_of(&self, name: &str) -> Result<String, CmdError> {
        let name = name.trim().trim_end_matches('.').to_ascii_lowercase();
        if name == self.name {
            return Ok("@".to_string());
        }
        name.strip_suffix(&format!(".{}", self.name))
            .map(str::to_string)
            .ok_or_else(|| {
                CmdError::usage(format!(
                    "{name:?} is not inside zone {:?}; name the zone with --zone",
                    self.name
                ))
            })
    }
}
