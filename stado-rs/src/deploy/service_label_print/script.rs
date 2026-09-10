//! The remote program this command sends, assembled from its head, its two
//! init-system branches and its tail — in that exact order, so the text a
//! host receives is the same program the branches spell out.

mod darwin;
mod linux;

/// The prologue every branch reads: the host's own init system, the validated
/// label, the bounded event predicate and the domain scope the caller asked
/// for.
const HEAD: &str = "set -u
os=$(/usr/bin/uname -s)
label=@LABEL@
predicate=@PREDICATE@
scope=@SCOPE@
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
/// `label` and `predicate` arrive already shell-quoted; `scope` is one of the
/// three domain words. Substitution happens here so no caller assembles this
/// program itself.
pub(super) fn label_print_script(label: &str, predicate: &str, scope: &str) -> String {
    format!("{HEAD}{}{}{TAIL}", darwin::BRANCH, linux::BRANCH)
        .replace("@LABEL@", label)
        .replace("@PREDICATE@", predicate)
        .replace("@SCOPE@", scope)
}
