//! The fixed argument shapes: the paths an entry's own arguments name, and
//! which of those are read from inside the managed account's home.

/// The managed service directory `com.wisent.weles-admission` runs out of,
/// relative to the managed account's home.
///
/// Written once, so the three entries that read it cannot drift apart about
/// which directory they are describing. `stado service release` installs
/// every release for a managed service under `.stado/services/<name>/` in a
/// directory named for the archive digest and points `current` at it, so this
/// prefix plus a digest is the whole of that service's installed history.
const WELES_ADMISSION_SERVICE_DIR: &str = ".stado/services/weles-admission";

/// Which installed release directory the admission unit executes through.
pub const WELES_ADMISSION_CURRENT: &[&str] = &[
    "/usr/bin/readlink",
    ".stado/services/weles-admission/current",
];

/// Every installed release directory for that service, with `current`'s own
/// target rendered beside it.
pub const WELES_ADMISSION_VERSIONS: &[&str] = &["/bin/ls", "-l", WELES_ADMISSION_SERVICE_DIR];

/// The compiled worker modules in the runtime tree that service's launcher
/// resolves — the directory `weles-api-server.mjs` imports `dispatch.js` from.
pub const WELES_ADMISSION_WORKER_MODULES: &[&str] = &[
    "/bin/ls",
    ".stado/services/weles-admission/current/darwin-arm/runtime/dist/worker",
];

/// What the launcher itself sees when it decides whether to unpack: the
/// payload archive, the derived `runtime` tree, and their timestamps.
pub const WELES_ADMISSION_RELEASE_TREE: &[&str] = &[
    "/bin/ls",
    "-l",
    ".stado/services/weles-admission/current/darwin-arm",
];
/// Size and modification epoch of the log the long-running manual Figma
/// export redirects away from Stado's canonical job log.
pub const FIGMA_EXPORT_LOG_STAT: &[&str] = &[
    "/usr/bin/stat",
    "-f",
    "%z:%m",
    ".stado/work/figma-export/export.log",
];

/// Total allocated KiB below the manual Figma export's fixed work tree.
pub const FIGMA_EXPORT_WORK_TREE_SIZE: &[&str] =
    &["/usr/bin/du", "-sk", ".stado/work/figma-export"];

/// Allocated size and open-file census for the two tagged Cargo trees that
/// dominate the MacBook's cleanup inventory. Both roots are fixed: an
/// operator cannot redirect either read at source or account data.
pub const JOB_PROGRESS_TARGET_SIZE: &[&str] = &[
    "/usr/bin/du",
    "-sk",
    "Documents/CodingProjects/Wisent/stado-job-progress-probe/stado-rs/target",
];
pub const JOB_PROGRESS_TARGET_OPEN_FILES: &[&str] = &[
    "/usr/sbin/lsof",
    "-n",
    "+D",
    "Documents/CodingProjects/Wisent/stado-job-progress-probe/stado-rs/target",
];
pub const WEB_HOSTING_TARGET_SIZE: &[&str] = &[
    "/usr/bin/du",
    "-sk",
    "Documents/CodingProjects/Wisent/stado-web-hosting/stado-rs/target",
];
pub const WEB_HOSTING_TARGET_OPEN_FILES: &[&str] = &[
    "/usr/sbin/lsof",
    "-n",
    "+D",
    "Documents/CodingProjects/Wisent/stado-web-hosting/stado-rs/target",
];

pub const BRAMA_RUNNER_APPHOST_SIGNATURE: &[&str] = &[
    "/usr/bin/codesign",
    "-d",
    "--entitlements",
    ":-",
    ".stado/actions-runner-brama/bin/Runner.Listener",
];
pub const BRAMA_RUNNER_CORECLR_SIGNATURE: &[&str] = &[
    "/usr/bin/codesign",
    "-dvvvv",
    ".stado/actions-runner-brama/bin/libcoreclr.dylib",
];

/// Prepare the one fixed parent under which Probierz run UUIDs live.
///
/// The operator can select this exact entry but cannot append a run name or
/// redirect it to another path. Individual canonical UUID children are made
/// by `stado host deliver`, which validates them before reaching the host.
pub const PROBIERZ_RUN_ROOT_CREATE: &[&str] = &["/bin/mkdir", "-p", ".stado/work/runs"];

/// Every entry whose fixed path arguments name something inside the managed
/// account's home rather than a system path.
///
/// Keyed on the entry's whole `argv`, the way [`super::PROGRAM_CANDIDATES`]
/// and [`crate::deploy::host_exec::channel::ACCOUNT_PROGRAMS`] are keyed on a
/// program: one more table beside the allowlist rather than one more field on
/// every entry in it.
///
/// Their paths are written RELATIVE and resolved by an explicit `cd "$HOME"`
/// in [`crate::deploy::host_exec::channel::home_rooted_script`], instead of
/// being spelled `~/…`. Barrier one of this module refuses any operator word
/// carrying a character a shell would act on, and `~` is one, so a `~/…`
/// argument would make its own entry unreachable through [`super::approve`] —
/// the operator could never type the spelling that selects it. The remote
/// login shell already starts in the managed account's home, so standing in
/// it changes nothing about where these reads land; it only stops the entry
/// from depending on that.
pub const HOME_ROOTED_READS: &[&[&str]] = &[
    WELES_ADMISSION_CURRENT,
    WELES_ADMISSION_VERSIONS,
    WELES_ADMISSION_RELEASE_TREE,
    WELES_ADMISSION_WORKER_MODULES,
    FIGMA_EXPORT_LOG_STAT,
    FIGMA_EXPORT_WORK_TREE_SIZE,
    JOB_PROGRESS_TARGET_SIZE,
    JOB_PROGRESS_TARGET_OPEN_FILES,
    WEB_HOSTING_TARGET_SIZE,
    WEB_HOSTING_TARGET_OPEN_FILES,
    BRAMA_RUNNER_APPHOST_SIGNATURE,
    BRAMA_RUNNER_CORECLR_SIGNATURE,
];

/// Is this entry's fixed argv one of the home-rooted reads?
pub fn home_rooted(argv: &[&str]) -> bool {
    HOME_ROOTED_READS.contains(&argv)
}

pub const MACOS_TAILSCALE_LOG_READ: &[&str] = &[
    "/usr/bin/log",
    "show",
    "--last",
    "1h",
    "--style",
    "compact",
    "--info",
    "--debug",
    "--no-pager",
    "--process",
    "Tailscale",
    "--process",
    "IPNExtension",
    "--process",
    "io.tailscale.ipn.macsys.network-extension",
    "--process",
    "tailscaled",
];

pub const LINUX_TAILSCALE_LOG_READ: &[&str] = &[
    "/usr/bin/journalctl",
    "--unit",
    "tailscaled",
    "--since",
    "-1h",
    "--no-pager",
    "--output",
    "short-iso",
];

#[cfg(test)]
mod tests {
    use super::super::{approve, APPROVED_COMMANDS};
    use super::*;
    use crate::deploy::host_exec::channel::home_rooted_script;

    /// The three service-tree reads name one service and no operator path.
    #[test]
    fn the_service_tree_reads_are_home_rooted_and_carry_no_absolute_path_argument() {
        for argv in HOME_ROOTED_READS {
            assert!(
                APPROVED_COMMANDS.iter().any(|entry| entry.argv == *argv),
                "{argv:?} is home-rooted but is not in the allowlist"
            );
            let (program, arguments) = argv.split_first().expect("a program");
            assert!(
                program.starts_with('/'),
                "{program} must be an absolute system program"
            );
            for argument in arguments {
                assert!(
                    !argument.starts_with('/') || argument.starts_with("-"),
                    "{argument} would escape the managed account's home"
                );
                assert!(
                    !argument.contains(".."),
                    "{argument} would climb out of the service tree"
                );
            }
        }
    }

    /// The reads that were unavailable on 2026-09-02 are the reads that now
    /// exist, addressed the way the running unit addresses the same tree.
    #[test]
    fn the_admission_reads_reach_current_the_way_the_unit_does() {
        assert_eq!(
            approve(&[
                "readlink".into(),
                ".stado/services/weles-admission/current".into()
            ])
            .expect("approved")
            .argv,
            WELES_ADMISSION_CURRENT
        );
        assert_eq!(
            approve(&[
                "ls".into(),
                "-l".into(),
                ".stado/services/weles-admission".into()
            ])
            .expect("approved")
            .argv,
            WELES_ADMISSION_VERSIONS
        );
        // Through `current`, not through a pinned digest: a read that named
        // the digest would answer for a tree the unit may not be running.
        assert!(
            WELES_ADMISSION_WORKER_MODULES[1]
                .starts_with(&format!("{WELES_ADMISSION_SERVICE_DIR}/current/")),
            "{:?}",
            WELES_ADMISSION_WORKER_MODULES
        );
        assert!(WELES_ADMISSION_WORKER_MODULES[1].ends_with("/runtime/dist/worker"));
    }

    #[test]
    fn a_home_rooted_read_stands_in_the_account_home_before_it_runs() {
        let script = home_rooted_script(WELES_ADMISSION_CURRENT);
        assert_eq!(
            script,
            "set -eu\ncd \"$HOME\"\nexec /usr/bin/readlink \
             .stado/services/weles-admission/current\n"
        );
    }
}
