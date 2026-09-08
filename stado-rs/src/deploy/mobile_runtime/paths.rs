//! The one candidate table this module and the allowlist share, rendered for
//! a remote shell, and the two homes repair installs into.

use crate::deploy::host_exec;
use crate::deploy::shlex_quote;

/// The npm prefix repair installs the Appium server under.
///
/// `~/.npm-global` is the first candidate [`crate::deploy::host_exec`]'s table
/// names for `appium`, so the program this installs is the program that
/// allowlist finds. A global install without an explicit prefix writes
/// wherever the host's npm happens to be configured, which on a Homebrew node
/// is a directory the fleet's probe order does not carry.
pub const NPM_PREFIX: &str = "$HOME/.npm-global";

/// Where repair unpacks Android platform-tools.
///
/// The first candidate the allowlist names for `adb`, and the location the
/// vendor's own SDK layout uses, so a later `sdkmanager` on this host manages
/// the same tree rather than a second copy.
pub const ANDROID_SDK_ROOT: &str = "$HOME/Library/Android/sdk";

/// One program's declared absolute paths, rendered as shell words for the
/// remote scripts.
///
/// Taken from [`crate::deploy::host_exec::program_candidates`] rather than
/// written out here, so the paths this module probes, installs into and
/// reports are the same paths the allowlist's own probe uses. Copying the list
/// would have let `stado host exec TARGET -- appium --version` and
/// `stado host mobile-runtime TARGET` disagree about which binary a host has.
///
/// A `~/`-anchored candidate becomes `"$HOME"/rest`, because only the host
/// knows what its login home is — the same expansion
/// [`crate::deploy::host_exec`]'s own `home_anchored` performs, and for the
/// same reason.
pub fn candidate_words(program: &str) -> String {
    let Some(candidates) = host_exec::program_candidates(program) else {
        // A program with one path is that path. Quoted, so a candidate table
        // that ever grows a space cannot split into two words.
        return shlex_quote(program);
    };
    candidates
        .iter()
        .map(|candidate| match candidate.strip_prefix("~/") {
            Some(rest) => format!("\"$HOME\"/{}", shlex_quote(rest)),
            None => shlex_quote(candidate),
        })
        .collect::<Vec<String>>()
        .join(" ")
}

/// Fill a remote script's candidate placeholders from the shared table.
pub(super) fn with_candidates(script: &str) -> String {
    script
        .replace(
            "@APPIUM_CANDIDATES@",
            &candidate_words(host_exec::APPIUM_PROGRAM),
        )
        .replace("@ADB_CANDIDATES@", &candidate_words(host_exec::ADB_PROGRAM))
        .replace(
            "@NODE_CANDIDATES@",
            &candidate_words(host_exec::NODE_PROGRAM),
        )
}
