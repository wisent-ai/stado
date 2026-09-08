//! The remote program for an entry whose binary this fleet installs at more
//! than one absolute path.

use crate::deploy::shlex_quote;

use super::super::RESOLVED_EXECUTABLE_MARKER;
use super::home_anchored;

/// The remote program for an entry whose binary is installed at a different
/// absolute path on each platform: run the first candidate that is executable
/// on the host, with the entry's own fixed arguments.
///
/// Every word of this script is a compile-time constant of this module — the
/// candidate paths and the entry's arguments — and each one is quoted for the
/// remote shell anyway. The operator's words selected the entry and reach the
/// host in nothing else, so barrier three of this module holds exactly as it
/// does on the [`crate::deploy::host_channel::run_program`] path.
///
/// A host carrying none of the candidates gets the refusal named here rather
/// than a shell's `No such file or directory` against whichever path happened
/// to be listed first, because the second reads as "the fleet installed this
/// wrongly" when the truth is "this program is not on this machine".
///
/// Every candidate's directory goes on `PATH` before the exec, and that is not
/// convenience. `/opt/homebrew/bin/npm` is a JavaScript shim whose first line
/// is `#!/usr/bin/env node`, so executing it on a channel whose `PATH` does
/// not carry Homebrew answers `env: node: No such file or directory` — which
/// is what `stado host exec charless-mac-mini -- npm --version` answered on
/// 2026-09-03 while `node --version` on the same host answered `v25.9.0` from
/// the directory beside it. The interpreter a shim needs is always a sibling
/// of the shim, so the directories this table already names are exactly the
/// ones that make it runnable. They are prepended, not appended: a host with
/// two Node installations must resolve the shim against the one whose path
/// this entry selected, not against whatever the login shell prefers.
pub fn candidate_script(candidates: &[&str], arguments: &[&str]) -> String {
    let fixed = arguments
        .iter()
        .map(|word| shlex_quote(word))
        .collect::<Vec<String>>()
        .join(" ");
    let mut script = String::from("set -eu\n");
    // Reversed, because each line prepends: emitting the candidates back to
    // front leaves the first candidate's directory first on PATH, which is the
    // same precedence the exec loop below applies.
    for candidate in candidates.iter().rev() {
        if let Some(directory) = std::path::Path::new(candidate).parent() {
            let directory = directory.to_string_lossy();
            if !directory.is_empty() {
                script.push_str(&format!(
                    "PATH={}:\"$PATH\"\n",
                    shlex_quote(directory.as_ref())
                ));
            }
        }
    }
    script.push_str("export PATH\n");
    for candidate in candidates {
        // A candidate may be home-relative: the installers that lay these
        // programs down (`~/.stado/bin/install-cua-driver`, rustup, a global
        // npm prefix) write into the login user's home, and only the host
        // knows what that path is. `home_anchored` expands nothing here — it
        // emits the host's own `"$HOME"` followed by the quoted remainder,
        // exactly as the account-owned entries already do.
        let path = home_anchored(candidate);
        let marker = shlex_quote(&format!("{RESOLVED_EXECUTABLE_MARKER}{candidate}"));
        script.push_str(&format!(
            "if [ -x {path} ]; then printf '%s\\n' {marker} >&2; exec {path} {fixed}; fi\n"
        ));
    }
    script.push_str(&format!(
        "printf '%s\\n' {} >&2\nexit 127\n",
        shlex_quote(&format!(
            "this program is installed at none of its approved paths on this host: {}",
            candidates.join(", ")
        ))
    ));
    script
}
