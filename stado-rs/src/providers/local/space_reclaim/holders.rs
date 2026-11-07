//! Which trees a running process holds right now: every path a live process
//! has open or works in (`lsof`) and every live process's arguments (`ps`),
//! each read once for a pass. The guard of every cleaner that takes a whole
//! tree no lock file protects: Xcode's project build directories and the
//! package caches.

use std::path::Path;
use std::process::Command;

/// The open-file tables, in the places the operating systems put them.
const LSOF: &[&str] = &["/usr/sbin/lsof", "/usr/bin/lsof"];

/// One snapshot of what live processes hold.
pub(in crate::providers::local::space_reclaim) struct Holders {
    open: Vec<String>,
    argv: String,
}

impl Holders {
    /// Read both tables, or the sentence saying which could not be read and
    /// why; the caller then removes nothing and adds that sentence to its
    /// pass's errors, where `stado space reclaim` shows it.
    pub(in crate::providers::local::space_reclaim) fn read() -> Result<Self, String> {
        let lsof = LSOF
            .iter()
            .find(|program| Path::new(program).is_file())
            .ok_or_else(|| {
                format!(
                    "no lsof at {}, so what live processes hold is not known; install lsof \
                     there and the next pass reads it",
                    LSOF.join(" or ")
                )
            })?;
        let open = output(lsof, &["-n", "-P", "-w", "-F", "n"])?
            .lines()
            .filter_map(|line| line.strip_prefix('n'))
            .map(str::to_string)
            .collect();
        let argv = output("/bin/ps", &["-Ao", "args="])?;
        Ok(Self { open, argv })
    }

    /// Whether a live process has a file under `tree` open, works in it, or
    /// names it in its arguments. The arguments are matched as text, so a
    /// process naming a longer path that begins with `tree` holds it too:
    /// the error keeps a tree, never takes one.
    pub(in crate::providers::local::space_reclaim) fn hold(&self, tree: &Path) -> bool {
        self.open
            .iter()
            .any(|name| Path::new(name).starts_with(tree))
            || self.argv.contains(tree.to_string_lossy().as_ref())
    }

    /// Whether a `cargo` or `rustc` process is running. A build reads its
    /// git dependencies' checkouts crate by crate, and between two compiler
    /// runs no process has one open or names it, so a checkout is safe to
    /// take only while no build runs at all.
    pub(in crate::providers::local::space_reclaim) fn cargo_runs(&self) -> bool {
        self.argv.lines().any(|line| {
            let program = line.split_whitespace().next().unwrap_or_default();
            let name = Path::new(program)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            name == "cargo" || name == "rustc" || name.starts_with("cargo-")
        })
    }
}

/// One command's standard output, or why it could not be read.
fn output(program: &str, args: &[&str]) -> Result<String, String> {
    match crate::wait::output(Command::new(program).args(args)) {
        // `lsof` exits 1 when one of the processes it lists vanished while
        // it read, and when it may not read another account's processes;
        // what it printed is still every file this account's processes have
        // open, and the trees it guards are this account's.
        Ok(output) if output.status.success() || !output.stdout.is_empty() => {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        Ok(output) => Err(format!(
            "{program} answered {} ({}), so what live processes hold is not known; once it \
             answers, the next pass reads it",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(error) => Err(format!(
            "{program} could not be run ({error}), so what live processes hold is not known; \
             once it runs, the next pass reads it"
        )),
    }
}
