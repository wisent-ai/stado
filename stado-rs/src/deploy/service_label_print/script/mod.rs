//! The remote program this command sends, assembled from its head, its two
//! init-system branches and its tail — in that exact order, so the text a
//! host receives is the same program the branches spell out.

mod darwin;
mod linux;

/// The prologue every branch reads: the host's own init system, the validated
/// label, the bounded event predicate, the domain scope the caller asked
/// for, and whether launchd's event log is read for this unit (`read`),
/// carried from the previous read (`carry`), or read only when the unit is
/// not as it was (`same:<key>`, the key the previous read printed).
const HEAD: &str = "set -u
os=$(/usr/bin/uname -s)
label=@LABEL@
predicate=@PREDICATE@
scope=@SCOPE@
events=@EVENTS@
uid=$(/usr/bin/id -u)
found=no
";

/// The epilogue: a host running neither init system names its OS instead of
/// answering, and every run ends by saying whether anything was found.
const TAIL: &str = "else
  printf 'STADO_LABEL_UNSUPPORTED\\t%s\\n' \"$os\"
fi
printf 'STADO_LABEL_DONE\\t%s\\n' \"$found\"
";

/// Read-only init-system query. Only named scalar properties, five explicitly
/// non-secret routing variables, one internally consistent running-image
/// identity, and a bounded exact-label event tail leave the host.
///
/// `label`, `predicate` and `events` arrive already shell-quoted; `scope` is
/// one of the three domain words. Substitution happens here so no caller
/// assembles this program itself.
pub(super) fn label_print_script(
    label: &str,
    predicate: &str,
    scope: &str,
    events: &str,
) -> String {
    format!(
        "{HEAD}{}{}{}{TAIL}",
        crate::deploy::host_disk::background_section(),
        darwin::BRANCH,
        linux::BRANCH
    )
    .replace("@LABEL@", label)
    .replace("@PREDICATE@", predicate)
    .replace("@SCOPE@", scope)
    .replace("@EVENTS@", events)
}
