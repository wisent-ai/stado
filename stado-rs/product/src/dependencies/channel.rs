//! How one line of a product's source reaches another product.
//!
//! Every name matched here is derived from the other product's catalog record
//! — its id and the programs its installations declare — so a product added to
//! the catalog is looked for without anyone listing it.

use serde::Serialize;

/// The way a source line reaches another product.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Channel {
    /// Dials the other product's service through a resolver forward marker
    /// (`forwards/<service>` or `<service>.local`).
    Resolver,
    /// Calls the other product's hosted origin (`<product>.wisent.com/...`);
    /// a link to its documentation is not a call.
    Http,
    /// Starts one of the other product's programs.
    Program,
    /// Reads or writes the other product's state directory (`.<product>/`).
    Files,
    /// Reads a variable in the other product's namespace (`<PRODUCT>_...`).
    Environment,
}

/// A standard-library call that starts a program whose quoted name follows
/// it on the same line.
#[derive(Clone, Copy)]
enum Spawn {
    RustCommand,
    NodeSpawn,
    NodeExecFile,
    PythonSubprocess,
    GoCommand,
}

impl Spawn {
    const ALL: &'static [Spawn] = &[
        Spawn::RustCommand,
        Spawn::NodeSpawn,
        Spawn::NodeExecFile,
        Spawn::PythonSubprocess,
        Spawn::GoCommand,
    ];

    /// The text that opens the call in source.
    fn opener(self) -> &'static str {
        match self {
            Spawn::RustCommand => "Command::new(",
            Spawn::NodeSpawn => "spawn(",
            Spawn::NodeExecFile => "execFile(",
            Spawn::PythonSubprocess => "subprocess.",
            Spawn::GoCommand => "exec.Command(",
        }
    }
}

/// The spellings by which source reaches one other product.
pub struct Names {
    /// The product's catalog id.
    pub id: String,
    programs: Vec<String>,
    forward: String,
    marker: String,
    origin: String,
    home: String,
    environment: String,
}

/// Whether the character before an occurrence ends a name: the occurrence is
/// the name itself, not the tail of a longer identifier.
fn starts_name(line: &str, at: usize) -> bool {
    line[..at]
        .chars()
        .next_back()
        .is_none_or(|before| !(before.is_alphanumeric() || before == '-' || before == '_'))
}

/// The text after every occurrence of `needle` that starts a name.
fn after_named<'a>(line: &'a str, needle: &'a str) -> impl Iterator<Item = &'a str> + 'a {
    line.match_indices(needle)
        .filter(move |(at, _)| starts_name(line, *at))
        .map(move |(at, found)| &line[at + found.len()..])
}

impl Names {
    /// The spellings of product `id`, whose installations declare `programs`.
    pub fn new(id: &str, programs: Vec<String>) -> Self {
        Self {
            id: id.to_owned(),
            programs,
            forward: format!("forwards/{id}"),
            marker: format!("{id}.local"),
            origin: format!("{id}.wisent.com/"),
            home: format!(".{id}/"),
            environment: format!("{}_", id.replace('-', "_").to_ascii_uppercase()),
        }
    }

    /// How `line` reaches this product, if it does. `shell` says the line is
    /// a shell script's, where a program is started by naming it first.
    pub fn channel(&self, line: &str, shell: bool) -> Option<Channel> {
        if line.contains(&self.forward) || after_named(line, &self.marker).next().is_some() {
            return Some(Channel::Resolver);
        }
        if after_named(line, &self.origin).any(|rest| !rest.starts_with("docs")) {
            return Some(Channel::Http);
        }
        if self
            .programs
            .iter()
            .any(|program| starts_program(line, program, shell))
        {
            return Some(Channel::Program);
        }
        if line
            .match_indices(&self.home)
            .any(|(at, _)| starts_name(line, at))
        {
            return Some(Channel::Files);
        }
        if after_named(line, &self.environment)
            .any(|rest| rest.starts_with(|next: char| next.is_ascii_uppercase()))
        {
            return Some(Channel::Environment);
        }
        None
    }
}

/// Whether `line` starts `program`: a spawn call naming it, a quoted path
/// ending in it, or, in a shell script, a command line beginning with it.
fn starts_program(line: &str, program: &str, shell: bool) -> bool {
    let quoted = format!("\"{program}\"");
    let path = format!("/{program}\"");
    let spawned =
        line.contains(&quoted) && Spawn::ALL.iter().any(|spawn| line.contains(spawn.opener()));
    let first = line.split_whitespace().next() == Some(program);
    spawned || line.contains(&path) || (shell && first)
}
