//! Where a signing scope's identity comes from, chosen once before anything
//! is built.
//!
//! The operator may name a source: a PEM pair (`WISENT_CODESIGN_CERTIFICATE_PEM`
//! with `WISENT_CODESIGN_PRIVATE_KEY_PEM`), a vault role
//! (`WISENT_CODESIGN_ROLE`), or a keychain of his own
//! (`WISENT_CODESIGN_KEYCHAIN`, or `--signing-keychain` on `stado product
//! install|update`). A named source is a choice, so the fleet's default
//! signing role ([`crate::Build::signing_role`]) never replaces it, and two
//! named sources are refused with both names. Naming only an identity
//! (`WISENT_CODESIGN_IDENTITY`, `--signing-identity`) chooses it from the
//! user's keychain search list. Only when nothing is named does the fleet role
//! apply, and its credential is read here, before the build: an unreachable
//! Skarbiec refuses in seconds and names the keychain choice, instead of after
//! a build of many minutes.

use super::secret;
use anyhow::{bail, Context, Result};
use std::path::PathBuf;

/// The variable naming the operator's keychain.
pub const KEYCHAIN_VARIABLE: &str = "WISENT_CODESIGN_KEYCHAIN";
/// The variable naming the identity, by its name or SHA-1.
pub const IDENTITY_VARIABLE: &str = "WISENT_CODESIGN_IDENTITY";
const ROLE_VARIABLE: &str = "WISENT_CODESIGN_ROLE";
const CERTIFICATE_VARIABLE: &str = "WISENT_CODESIGN_CERTIFICATE_PEM";
const PRIVATE_KEY_VARIABLE: &str = "WISENT_CODESIGN_PRIVATE_KEY_PEM";
const ISSUERS_VARIABLE: &str = "WISENT_CODESIGN_ISSUERS_FILE";

/// A signing source, resolved and, for a role, already read.
pub enum Source {
    /// A certificate and key in hand: supplied as PEM, or read from the item
    /// that plays a role. `from` says which, for refusals.
    Material {
        certificate: String,
        private_key: String,
        from: String,
    },
    /// A keychain the operator named; its identity is his to choose.
    Keychain(PathBuf),
    /// The user's keychain search list: an identity was named without a
    /// keychain, or no fleet role is built in.
    SearchList,
}

/// The keychain and identity an invocation named on its command line
/// (`--signing-keychain`, `--signing-identity`), carried by the product
/// runtime to every build and placement the invocation runs. Each is the
/// choice its variable states.
#[derive(Clone, Default)]
pub struct Choice {
    keychain: Option<String>,
    identity: Option<String>,
}

fn environment(variable: &str) -> Option<String> {
    std::env::var(variable)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

impl Choice {
    /// The choice a command line made. A variable already set to another
    /// value beside its flag is two answers, refused naming both.
    pub fn new(keychain: Option<&str>, identity: Option<&str>) -> Result<Self> {
        for (value, flag, variable) in [
            (keychain, "--signing-keychain", KEYCHAIN_VARIABLE),
            (identity, "--signing-identity", IDENTITY_VARIABLE),
        ] {
            if let (Some(value), Some(set)) = (value, environment(variable)) {
                if set != value {
                    bail!("{flag} {value} and {variable}={set} name two signing choices; name one");
                }
            }
        }
        Ok(Self {
            keychain: keychain.map(str::to_owned),
            identity: identity.map(str::to_owned),
        })
    }

    /// The choice `--signing-keychain` and `--signing-identity` make on a
    /// `stado product install|update` command line.
    pub fn from_arguments(args: &crate::common::Arguments) -> Result<Self> {
        Self::new(
            args.optional("--signing-keychain")?,
            args.optional("--signing-identity")?,
        )
    }

    /// The keychain named here, else in [`KEYCHAIN_VARIABLE`].
    fn keychain(&self) -> Option<String> {
        self.keychain
            .clone()
            .or_else(|| environment(KEYCHAIN_VARIABLE))
    }

    /// The identity `requested` by a caller, else named here, else in
    /// [`IDENTITY_VARIABLE`].
    pub fn identity(&self, requested: Option<&str>) -> Option<String> {
        requested
            .map(str::to_owned)
            .or_else(|| self.identity.clone())
            .or_else(|| environment(IDENTITY_VARIABLE))
    }
}

impl Source {
    /// Resolve the source from `choice`, the environment and `requested`,
    /// the identity a caller named on its own command line.
    pub fn resolve(choice: &Choice, requested: Option<&str>) -> Result<Self> {
        let certificate = environment(CERTIFICATE_VARIABLE);
        let private_key = environment(PRIVATE_KEY_VARIABLE);
        let role = environment(ROLE_VARIABLE);
        let keychain = choice.keychain();
        let pem = match (certificate, private_key) {
            (Some(certificate), Some(private_key)) => Some((certificate, private_key)),
            (None, None) => None,
            _ => bail!("provide both {CERTIFICATE_VARIABLE} and {PRIVATE_KEY_VARIABLE}"),
        };
        let chosen: Vec<&str> = [
            pem.as_ref().map(|_| CERTIFICATE_VARIABLE),
            role.as_ref().map(|_| ROLE_VARIABLE),
            keychain.as_ref().map(|_| KEYCHAIN_VARIABLE),
        ]
        .into_iter()
        .flatten()
        .collect();
        if let [first, second, ..] = chosen.as_slice() {
            bail!(
                "{first} and {second} each name a signing source; name one: the signing \
                 identity comes from exactly one place"
            );
        }
        let source = if let Some((certificate, private_key)) = pem {
            Self::Material {
                certificate,
                private_key,
                from: format!("{CERTIFICATE_VARIABLE} and {PRIVATE_KEY_VARIABLE}"),
            }
        } else if let Some(role) = role {
            Self::read_role(&role, ROLE_VARIABLE)?
        } else if let Some(keychain) = keychain {
            let path = PathBuf::from(&keychain);
            Self::Keychain(path.canonicalize().with_context(|| {
                format!("{KEYCHAIN_VARIABLE} names the keychain {keychain}, which cannot be read")
            })?)
        } else if choice.identity(requested).is_some() {
            Self::SearchList
        } else {
            let fleet = crate::build().signing_role;
            if fleet.is_empty() {
                Self::SearchList
            } else {
                Self::read_role(fleet, "the fleet's default signing role")?
            }
        };
        source.with_issuers()
    }

    /// Read the certificate and key of the item that plays `role`. A refusal
    /// names the keychain choice, which needs no vault.
    fn read_role(role: &str, chosen_by: &str) -> Result<Self> {
        if role.contains('#') {
            bail!("{ROLE_VARIABLE} names a role, not a role#field coordinate");
        }
        let read = |field: &str| {
            secret(role, field).with_context(|| {
                format!(
                    "the signing credential was read before building, and no build was \
                     started; role {role} is {chosen_by}. To sign with a keychain of your \
                     own instead, name it: --signing-keychain <keychain> (or \
                     {KEYCHAIN_VARIABLE}), with --signing-identity <name or SHA-1> (or \
                     {IDENTITY_VARIABLE}) when it holds more than one valid identity; \
                     `security find-identity -v -p codesigning <keychain>` lists them"
                )
            })
        };
        Ok(Self::Material {
            certificate: read("certificate")?,
            private_key: read("private_key")?,
            from: format!("role {role} ({chosen_by})"),
        })
    }

    /// Append the supplied issuers file to a certificate in hand.
    fn with_issuers(self) -> Result<Self> {
        let Some(issuers) = std::env::var_os(ISSUERS_VARIABLE) else {
            return Ok(self);
        };
        let Self::Material {
            mut certificate,
            private_key,
            from,
        } = self
        else {
            bail!("{ISSUERS_VARIABLE} requires a certificate in hand (a PEM pair or a role)");
        };
        let issuers = std::fs::read_to_string(&issuers).with_context(|| {
            format!(
                "reading the signing issuers {ISSUERS_VARIABLE} names: {}",
                PathBuf::from(&issuers).display()
            )
        })?;
        super::blocks(&issuers)?;
        certificate.push_str(&format!("\n{issuers}"));
        Ok(Self::Material {
            certificate,
            private_key,
            from,
        })
    }

    /// Whether the operator named this keychain, so its identity is chosen
    /// only by the name or digest he gave, or as its one valid identity.
    pub fn operator_keychain(&self) -> bool {
        matches!(self, Self::Keychain(_))
    }
}
