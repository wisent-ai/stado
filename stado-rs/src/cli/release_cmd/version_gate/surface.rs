//! The public top-level command surface a CLI binary advertises: the names
//! under `Commands:` in its `help`, `help` itself excluded. Hidden commands
//! are not advertised, so they are not part of the contract.

use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;

use regex::Regex;

static COMMAND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^  ([a-z0-9][a-z0-9-]*)\s{2,}\S").expect("static"));

/// The sorted, de-duplicated names; an empty section is refused rather than
/// reported as a surface that lost every command.
pub(super) fn advertised(help: &str) -> Result<Vec<String>, String> {
    let mut commands = Vec::new();
    let mut in_commands = false;
    for line in help.lines() {
        if line == "Commands:" {
            in_commands = true;
            continue;
        }
        if !in_commands {
            continue;
        }
        if !line.is_empty() && !line.starts_with(' ') {
            break;
        }
        if let Some(found) = COMMAND.captures(line) {
            if &found[1] != "help" {
                commands.push(found[1].to_string());
            }
        }
    }
    if commands.is_empty() {
        return Err("CLI help contains no advertised Commands section".into());
    }
    commands.sort();
    commands.dedup();
    Ok(commands)
}

/// Ask the artifact itself: `BINARY help`.
pub(super) fn of_binary(binary: &Path) -> Result<Vec<String>, String> {
    let output = Command::new(binary)
        .arg("help")
        .output()
        .map_err(|error| format!("{}: {error}", binary.display()))?;
    if !output.status.success() {
        return Err(format!(
            "{} help exited {}: {}",
            binary.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    advertised(&String::from_utf8_lossy(&output.stdout))
}

/// `{"surface": [...]}`, pretty-printed as the gate's documents are.
pub(super) fn document(commands: &[String]) -> String {
    serde_json::to_string_pretty(&serde_json::json!({ "surface": commands }))
        .expect("a list of strings serialises")
}
