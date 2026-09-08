//! `service env-set`, `env-unset` and `env-show`: one key of one managed
//! unit's owner-controlled environment, written and then read back through
//! the channel that wrote it.
//!
//! [`ReadBack`] is that read's verdict, and [`validate_env_key`] the one rule
//! every writer here applies before a host is contacted.

use super::*;

pub(crate) mod set;
pub(crate) mod show;
pub(crate) mod unset;
mod verify;

use verify::{verify_env_write, verify_unit_env_write};

fn validate_env_key(key: &str) -> Result<(), CmdError> {
    if key.is_empty()
        || key
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_digit())
        || !key.chars().all(|character| {
            character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
        })
    {
        return Err(CmdError::click(
            "--key must be an uppercase environment variable name",
        ));
    }
    Ok(())
}

/// What the env file said about one key immediately after it was written.
struct ReadBack {
    /// [`service_env_file::EXPECT_MATCHED`], `_DIFFERS`, `_ABSENT` or
    /// `_UNVERIFIED`.
    state: &'static str,
    /// The value the file actually holds now, when the host was willing to
    /// show it. `None` for a withheld value, whose length is still reported.
    effective: Option<String>,
    /// How long that value is, shown or withheld.
    chars: u32,
    /// `name (path)` of the forward marker that holds exactly the value which
    /// replaced ours, when one does. This is the declaration to correct.
    marker: Option<String>,
}

impl ReadBack {
    /// The `EFFECTIVE` column: the value, or its length when it is withheld.
    fn effective_cell(&self) -> String {
        match &self.effective {
            Some(value) => value.clone(),
            None if self.state == service_env_file::EXPECT_MATCHED => "-".to_string(),
            None => format!("<withheld, {} chars>", self.chars),
        }
    }

    /// The refusal for a write that did not survive, or `None` when it did.
    ///
    /// It names the marker whenever one holds exactly what came back, because
    /// the operator's next move is to correct that declaration, not to write
    /// this file again — which is what happened twice before this check
    /// existed. With no marker to name it points at the command that
    /// enumerates every unit that could be the writer, rather than shrugging.
    fn failure(&self, host: &str, key: &str) -> Option<String> {
        let observed = match &self.effective {
            Some(value) => format!("{value:?}"),
            None => format!("a withheld {}-character value", self.chars),
        };
        match self.state {
            service_env_file::EXPECT_MATCHED => None,
            service_env_file::EXPECT_DIFFERS => Some(match &self.marker {
                Some(marker) => format!(
                    "{host}: {key} was replaced after the write and now holds {observed}, \
                     which is exactly what the forward marker {marker} declares — correct \
                     that marker, not this file"
                ),
                None => format!(
                    "{host}: {key} was replaced after the write and now holds {observed}; \
                     something on the host owns this key. `stado service list --undeclared` \
                     names every unit that could"
                ),
            }),
            service_env_file::EXPECT_ABSENT => Some(format!(
                "{host}: {key} is assigned nowhere in the file after the write; something \
                 on the host removed it"
            )),
            _ => Some(format!(
                "{host}: the write reported success and could not be read back, so whether \
                 {key} survived is unknown"
            )),
        }
    }
}
