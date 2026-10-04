//! `after_install`: the commands a catalog recipe runs once its product is
//! installed, with the placeholders that tie a command to this installation.
//!
//! - `{release_archive}` and `{release_archive_sha256}` name the verified
//!   archive this installation came from, so a step can hand the exact bytes
//!   to the product's own reconciler: Stado's `release converge-local-readers`
//!   restarts every unit still executing the binary this install replaced.
//! - `{host}` is the registry name of the host this installation placed the
//!   product on, wherever it stands in a word.
//! - `{install_id:NAME}` is a UUID derived from the product, the surface, the
//!   host and NAME: the same on every run of the same installation, different
//!   for every other one. It is what a step that declares lasting state passes
//!   as that state's identity, so declaring it again on the next install
//!   changes nothing (`stado schedule create --id {install_id:maintain}`).
//!
//! A step runs a binary the installation produced, or Stado itself: a product
//! whose installation has to declare something in Stado (a gateway's
//! maintenance schedule pinned to its host) names `stado` as the program, and
//! the Stado running this installation runs it.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::process::Command;

use crate::common::{checked, stado};
use crate::state::ProductState;

const INSTALL_ID: &str = "{install_id:";

pub(super) fn run(
    id: &str,
    surface: &str,
    host: Option<&str>,
    steps: &Value,
    installed: &ProductState,
) -> Result<Vec<Value>> {
    let archive = installed
        .release
        .as_ref()
        .and_then(|release| release["destination"].as_str())
        .map(str::to_owned);
    let archive_sha256 = installed
        .release
        .as_ref()
        .and_then(|release| release["artifact"]["artifact_sha256"].as_str())
        .map(str::to_owned);
    let mut outcomes = Vec::new();
    for step in steps.as_array().context("after_install must be an array")? {
        let words = step
            .as_array()
            .context("after_install command must be argv")?
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .context("after_install arguments must be strings")
            })
            .collect::<Result<Vec<_>>>()?;
        // A step that hands the release archive to a reconciler has nothing to
        // hand when the installation was built from source. An option whose
        // value is a release placeholder is then left out, so the reconciler
        // still runs on what the source install did place (Stado's recycles
        // the units executing the binary it replaced); a placeholder in any
        // other position cannot be dropped, and that step is recorded as not
        // run, with the reason, over a working install.
        let is_placeholder =
            |word: &str| matches!(word, "{release_archive}" | "{release_archive_sha256}");
        let words: Vec<&str> = if archive.is_none() {
            let mut kept = Vec::with_capacity(words.len());
            let mut index = 0;
            while index < words.len() {
                let is_option = words[index].starts_with("--");
                if is_option
                    && words
                        .get(index + 1)
                        .is_some_and(|value| is_placeholder(value))
                {
                    index += 2;
                    continue;
                }
                kept.push(words[index]);
                index += 1;
            }
            kept
        } else {
            words
        };
        if archive.is_none() && words.iter().any(|word| is_placeholder(word)) {
            let reason = "this installation was built from source, so there is no verified \
                          release archive to hand to the step; readers it would reconcile \
                          keep their image until a release install";
            eprintln!("{id}: after_install step {words:?} not run: {reason}");
            outcomes.push(json!({"argv": words, "ran": false, "reason": reason}));
            continue;
        }
        let argv = words
            .iter()
            .map(|word| match *word {
                "{release_archive}" => archive.clone().context(
                    "after_install names {release_archive}, and this installation came from \
                     no verified release archive",
                ),
                "{release_archive_sha256}" => archive_sha256.clone().context(
                    "after_install names {release_archive_sha256}, and this installation came \
                     from no verified release archive",
                ),
                word => substitute(word, id, surface, host),
            })
            .collect::<Result<Vec<_>>>()?;
        let (name, arguments) = argv
            .split_first()
            .context("after_install has an empty command")?;
        let produced = installed
            .installed_paths
            .iter()
            .find(|path| path.file_name().and_then(|s| s.to_str()) == Some(name.as_str()));
        let mut command = match produced {
            Some(binary) => Command::new(binary),
            None if name == "stado" => stado(),
            None => bail!(
                "after_install names a binary this installation did not produce: {name}; a step \
                 runs one of the installation's own binaries, or stado"
            ),
        };
        checked(command.args(arguments))?;
        outcomes.push(json!({"argv": argv, "ran": true}));
    }
    Ok(outcomes)
}

/// `{host}` and every `{install_id:NAME}` in one word.
fn substitute(word: &str, id: &str, surface: &str, host: Option<&str>) -> Result<String> {
    let mut word = if word.contains("{host}") {
        let host = host.context(
            "after_install names {host}, and this installation places the product on no host",
        )?;
        word.replace("{host}", host)
    } else {
        word.to_owned()
    };
    while let Some(start) = word.find(INSTALL_ID) {
        let rest = &word[start + INSTALL_ID.len()..];
        let end = rest.find('}').with_context(|| {
            format!("after_install word `{word}` opens {INSTALL_ID} and never closes it")
        })?;
        let name = &rest[..end];
        if name.is_empty() {
            bail!("after_install word `{word}` names an install id without a name");
        }
        let identity = install_id(id, surface, host.unwrap_or_default(), name);
        word.replace_range(start..start + INSTALL_ID.len() + end + 1, &identity);
    }
    Ok(word)
}

/// The UUID one installation declares the state called `name` under.
fn install_id(product: &str, surface: &str, host: &str, name: &str) -> String {
    let digest = Sha256::digest(
        format!("stado.after_install.v1\0{product}\0{surface}\0{host}\0{name}").as_bytes(),
    );
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Builder::from_custom_bytes(bytes)
        .into_uuid()
        .hyphenated()
        .to_string()
}
