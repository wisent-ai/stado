//! Why a scheme-less path reached the store it reached.

/// The sentence a bare path earns when the store refuses it.
///
/// `ObjectRef::parse` accepts `<namespace>/<key>` as well as
/// `stado://<namespace>/<key>`, so a path without the scheme still names a
/// namespace — the FIRST SEGMENT — and the probe goes wherever that points.
/// For release objects this is a trap with no tell, because a release key
/// itself begins with the product name: `stado/0.13.20/darwin-arm64/SHA256SUMS`
/// reads like a complete coordinate and is actually namespace `stado`, the
/// queue store, where the caller has no grant. The object API then answers
/// `HTTP 401 {"error":"unauthorized"}` — a true statement about a namespace
/// nobody meant to ask about, which `CmdError` classifies as `auth` and
/// reports as "the credentials this command used were rejected".
///
/// On 2026-09-01 that cost an investigation into a credential that was never
/// broken: the same object answered `present` through
/// `stado://releases/stado/0.13.20/darwin-arm64/SHA256SUMS` in the same second,
/// and `/api/release/object` served it unauthenticated. The refusal was
/// correct; only its attribution was wrong.
///
/// So the hint is attached to the refusal rather than the state: the exit-code
/// contract still reports `unreachable`, because the store this path named
/// genuinely did not answer for it.
pub(in crate::cli::storage) fn inferred_namespace_hint(path: &str) -> String {
    if path.starts_with("stado://") {
        return String::new();
    }
    let Some((namespace, key)) = path.split_once('/') else {
        return String::new();
    };
    if namespace.is_empty() || key.is_empty() {
        return String::new();
    }
    format!(
        " This path has no `stado://` scheme, so its first segment was read as \
         the namespace: it asked the {namespace:?} store about {key:?}, which is \
         probably not what you meant. Name the namespace explicitly — \
         `stado://<namespace>/<key>` — and note that published release objects \
         live under `stado://releases/`, whose keys start with the product \
         name: `stado://releases/{path}`."
    )
}
