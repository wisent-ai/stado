//! The program search path a fleet unit or a command Stado runs on a host
//! carries, declared once. A launchd agent starts with the system's bare
//! path, which holds none of the directories Homebrew installs into, so
//! every unit and remote command Stado writes names this one path instead of
//! its own spelling: a tool is then found by every Stado operation on a host
//! or by none of them.

/// Homebrew's prefix on Apple silicon, then Homebrew's prefix on Intel Macs
/// and the conventional local prefix elsewhere, then the system directories.
pub const SEARCH_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

/// The same path as a shell assignment, for a command line a host runs.
pub fn assignment() -> String {
    format!("PATH={SEARCH_PATH}")
}

/// [`assignment`] for a command that runs Skarbiec on a host as `home`'s
/// account: the fleet's path, then the directory GPG Suite installs `gpg`
/// into on a Mac without Homebrew's, then the account's own `~/.local/bin`.
/// Every Skarbiec command Stado runs on a host takes this one path, so `gpg`
/// is found by all of them or by none.
pub fn vault_assignment(home: &str) -> String {
    format!("{}:/usr/local/MacGPG2/bin:{home}/.local/bin", assignment())
}
