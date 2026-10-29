//! Reads of what the operating system's privacy protection asked or refused
//! on a host: the evidence a person's own Mac needs when Stado runs there.

use super::super::ApprovedCommand;

/// The last hour of macOS's privacy daemon log, as it was retained.
pub const MACOS_TCC_LOG_READ: &[&str] = &[
    "/usr/bin/log",
    "show",
    "--last",
    "1h",
    "--style",
    "compact",
    "--info",
    "--no-pager",
    "--process",
    "tccd",
];

pub const PRIVACY_READS: &[ApprovedCommand] = &[ApprovedCommand {
    argv: MACOS_TCC_LOG_READ,
    why: "reads the last hour of retained macOS logs from tccd, the daemon that grants, \
          refuses and prompts for privacy access (Documents, Desktop, Downloads, local \
          network, Full Disk Access), naming the process that asked. On a shared Mac this \
          is how a prompt Stado raised is seen without asking its owner. The process, \
          window and format are fixed; the read changes no setting, prompts for nothing \
          and enables no logging",
}];
