//! Which catalog product a unit belongs to, read off what the unit runs.
//!
//! A product runs as one process per host under its one unit. Any other unit
//! that runs that product's program is work the product's process replaced:
//! an older label, a split role, a copy somebody bootstrapped by hand. The
//! catalog used to name such units product by product, which needed a new
//! entry and a new release for every old unit found on a host and still
//! missed the next one. The program is the fact that identifies a product
//! whatever its unit is called, so ownership is decided here from the
//! program, and no unit name is listed anywhere.

use super::{all, executable_name, resolve_word, CatalogService, RoleUnit, API_TAKEOVER};
use clap::CommandFactory;

/// The first words that only start another program: the program the unit
/// really runs is the first operand after them.
const INTERPRETERS: &[&str] = &[
    "sh", "bash", "zsh", "dash", "env", "node", "bun", "deno", "python", "python3", "perl", "ruby",
];

/// The unit a catalog entry runs as: its launchd label, or, by the catalog's
/// rule for an entry that declares none, its product name.
pub fn unit_of(entry: &CatalogService) -> &str {
    match entry.unit.as_deref() {
        Some(unit) => unit,
        None => &entry.name,
    }
}

/// Whether `label` names `entry`'s own unit: its product name, its launchd
/// label, or that label as a systemd unit.
pub fn owns_label(entry: &CatalogService, label: &str) -> bool {
    let unit = unit_of(entry);
    label == entry.name || label == unit || label.strip_suffix(".service") == Some(unit)
}

/// Whether `label` is some catalog product's own unit.
pub fn is_catalog_unit(label: &str) -> Result<bool, String> {
    Ok(all()?.iter().any(|entry| owns_label(entry, label)))
}

/// `product`'s catalog entry when `label`, named under `product`, cannot be
/// its unit: a product runs as one unit per host, so a label filed under a
/// product that is neither that product's unit nor any other product's is a
/// label the product ran under before. `None` when the product declares no
/// service, when `label` is its unit, and when `label` is another product's.
pub fn superseded_label(product: &str, label: &str) -> Result<Option<CatalogService>, String> {
    let entries = all()?;
    if entries.iter().any(|entry| owns_label(entry, label)) {
        return Ok(None);
    }
    Ok(entries.into_iter().find(|entry| entry.name == product))
}

/// The words of a command line that can be the program it runs: its first
/// word and, when that word is an interpreter, the first operand after it.
pub fn program_words(line: &str) -> Vec<&str> {
    let mut words = line.split_whitespace();
    let Some(head) = words.next() else {
        return Vec::new();
    };
    let mut programs = vec![head];
    if executable_name(head).is_some_and(|name| INTERPRETERS.contains(&name)) {
        if let Some(operand) = words.find(|word| !word.starts_with('-') && !word.contains('=')) {
            programs.push(operand);
        }
    }
    programs
}

/// The directory a catalog program is installed into when that directory
/// is its own: `<home>/.stado/services/<name>/` for a program under the
/// deployed-services root. `None` for a program in a directory several
/// products share, such as `~/.stado/bin`, where only the file name says
/// which product it is.
fn own_tree(entry: &CatalogService, home: &str, platform: &str, host: &str) -> Option<String> {
    let root = resolve_word(
        crate::deploy::service::DEPLOYED_SERVICES_ROOT,
        home,
        platform,
        host,
    );
    let program = resolve_word(&entry.program, home, platform, host);
    let name = program
        .strip_prefix(&format!("{root}/"))?
        .split('/')
        .next()?;
    (!name.is_empty()).then(|| format!("{root}/{name}/"))
}

/// Whether a unit whose declaration runs `declared` and whose live process
/// runs `running` runs `entry`'s program on the host whose home, release
/// platform and registry name are given: a program in `entry`'s own tree; in a
/// directory several products share, the declared program itself, or any
/// file named after the product wherever it was installed; for the host Stado
/// process, also one of the programs Stado builds beside it.
pub fn runs_program_of(
    entry: &CatalogService,
    declared: &str,
    running: &str,
    home: &str,
    platform: &str,
    host: &str,
) -> bool {
    let tree = own_tree(entry, home, platform, host);
    let resolved = resolve_word(&entry.program, home, platform, host);
    let named = executable_name(&entry.program).filter(|name| *name == entry.name);
    program_words(declared)
        .into_iter()
        .chain(program_words(running))
        .any(|word| match &tree {
            Some(tree) => word.starts_with(tree.as_str()),
            None => {
                let name = executable_name(word);
                word == resolved || (named.is_some() && name == named)
            }
        })
}

/// The catalog product a unit labelled `label` belongs to although it is not
/// that product's unit: the one whose program it runs. `None` for a catalog
/// unit itself and for a unit that runs no catalog product's program.
pub fn owner_of(
    label: &str,
    declared: &str,
    running: &str,
    home: &str,
    platform: &str,
    host: &str,
) -> Result<Option<CatalogService>, String> {
    let entries = all()?;
    if entries.iter().any(|entry| owns_label(entry, label)) {
        return Ok(None);
    }
    Ok(entries
        .into_iter()
        .find(|entry| runs_program_of(entry, declared, running, home, platform, host)))
}

/// The `stado serve` roles a command line of `entry`'s program does the work
/// of: the words after the program, read by Stado's own command definitions.
/// Empty when the line runs neither or runs no role.
pub fn host_roles(entry: &CatalogService, line: &str) -> Vec<&'static str> {
    let Some(executable) = executable_name(&entry.program) else {
        return Vec::new();
    };
    let words: Vec<&str> = line.split_whitespace().collect();
    let mut rest = words.as_slice();
    while let Some((word, after)) = rest.split_first() {
        if executable_name(word) == Some(executable) {
            return crate::cli::integrations::runtime::roles::command_roles(after);
        }
        rest = after;
    }
    Vec::new()
}

/// Whether the program's root command still exists in this release. Removed
/// verbs have no work the current binary can perform, unlike valid one-pass
/// commands and root options that deliberately have no resident serve role.
pub fn host_command_known(entry: &CatalogService, line: &str) -> bool {
    let Some(executable) = executable_name(&entry.program) else {
        return false;
    };
    let mut words = line.split_whitespace();
    if words
        .find(|word| executable_name(word) == Some(executable))
        .is_none()
    {
        return false;
    }
    let Some(verb) = words.next() else {
        return false;
    };
    let command = crate::cli::entry::spec::Cli::command();
    if command.find_subcommand(verb).is_some() {
        return true;
    }
    match command.try_get_matches_from([executable, verb]) {
        Ok(_) => true,
        Err(error) => matches!(
            error.kind(),
            clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
        ),
    }
}

/// The role unit `label` is when it does the work of `roles`: the API
/// listener's takeover proves an API unit, the resolver's published state a
/// resolver unit, and the live process's role options every other one.
/// `None` for no roles.
pub fn role_unit(label: &str, roles: &[&str]) -> Option<RoleUnit> {
    let primary = ["--api", "--resolver"]
        .into_iter()
        .find(|flag| roles.contains(flag))
        .or_else(|| roles.first().copied())?;
    let readiness = match primary {
        "--api" => Some(API_TAKEOVER.to_string()),
        "--resolver" => Some(crate::deploy::service::RESOLVER_STATE.to_string()),
        _ => None,
    };
    Some(RoleUnit {
        unit: label.to_string(),
        flag: primary.to_string(),
        also: roles
            .iter()
            .filter(|role| **role != primary)
            .map(|role| role.to_string())
            .collect(),
        readiness,
    })
}
